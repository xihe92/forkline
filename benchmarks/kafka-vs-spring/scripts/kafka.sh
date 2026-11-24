#!/usr/bin/env bash
# Start or stop the benchmark's local Kafka cluster (Homebrew's `kafka`).
#
#   scripts/kafka.sh start      # 1 broker; topic `events` with 6 partitions, 1 replica
#   KAFKA_BROKERS=3 KAFKA_PARTITIONS=10 KAFKA_RF=3 scripts/kafka.sh start
#   scripts/kafka.sh stop
#
# Every node is a broker and a controller. Node n listens on 127.0.0.1:(9090+2n)
# for clients (9092, 9094, 9096, ...) and on the next port for the controller
# quorum. KAFKA_MIN_ISR defaults to 2 when there are 3 or more replicas, else 1.
# KAFKA_JMX_PORT=9999 also exposes each node's metrics over JMX, on 9999, 10000, ...
# Changing the cluster's shape wipes target/kafka-data (benchmark data only).
set -euo pipefail
unset JMX_PORT
cd "$(dirname "$0")/.."

BROKERS="${KAFKA_BROKERS:-1}"
PARTITIONS="${KAFKA_PARTITIONS:-6}"
RF="${KAFKA_RF:-1}"
if [ "$RF" -ge 3 ]; then MIN_ISR_DEFAULT=2; else MIN_ISR_DEFAULT=1; fi
MIN_ISR="${KAFKA_MIN_ISR:-$MIN_ISR_DEFAULT}"
INTERNAL_RF=$(( BROKERS < 3 ? BROKERS : 3 ))
DATA="$PWD/target/kafka-data"
SHAPE="brokers=$BROKERS partitions=$PARTITIONS rf=$RF min_isr=$MIN_ISR"
BOOT=127.0.0.1:9092

start() {
  if [ "$RF" -gt "$BROKERS" ]; then
    echo "KAFKA_RF ($RF) cannot exceed KAFKA_BROKERS ($BROKERS)" >&2
    exit 2
  fi
  mkdir -p target
  if [ -f "$DATA/shape" ] && [ "$(cat "$DATA/shape")" != "$SHAPE" ]; then
    echo "cluster shape changed; wiping $DATA"
    rm -rf "$DATA"
  fi

  local voters="" n
  for n in $(seq "$BROKERS"); do
    voters="${voters:+$voters,}$n@127.0.0.1:$(( 9091 + 2 * n ))"
  done

  local cluster_id=""
  [ -f "$DATA/cluster-id" ] && cluster_id="$(cat "$DATA/cluster-id")"
  if [ -z "$cluster_id" ]; then
    mkdir -p "$DATA"
    cluster_id="$(kafka-storage random-uuid)"
    echo "$cluster_id" > "$DATA/cluster-id"
    echo "$SHAPE" > "$DATA/shape"
  fi

  local jmx_ports=""
  for n in $(seq "$BROKERS"); do
    local conf="$PWD/target/kafka-$n.properties" dir="$DATA/node-$n"
    sed -e "s|@NODE_ID@|$n|" -e "s|@VOTERS@|$voters|" \
        -e "s|@BROKER_PORT@|$(( 9090 + 2 * n ))|" -e "s|@CONTROLLER_PORT@|$(( 9091 + 2 * n ))|" \
        -e "s|@DATA_DIR@|$dir|" -e "s|@PARTITIONS@|$PARTITIONS|" -e "s|@RF@|$RF|" \
        -e "s|@INTERNAL_RF@|$INTERNAL_RF|" kafka/server.properties > "$conf"
    if [ ! -f "$dir/meta.properties" ]; then
      kafka-storage format -t "$cluster_id" -c "$conf" > /dev/null
    fi
    local jmx=""
    if [ -n "${KAFKA_JMX_PORT:-}" ]; then
      jmx=$(( KAFKA_JMX_PORT + n - 1 ))
      jmx_ports="${jmx_ports:+$jmx_ports,}$jmx"
    fi
    # Only the broker gets JMX_PORT: Kafka's CLI tools also read it and would
    # try to bind the same port. LOG_DIR keeps each node's logs apart.
    mkdir -p "target/kafka-logs/node-$n"
    JMX_PORT="$jmx" LOG_DIR="$PWD/target/kafka-logs/node-$n" KAFKA_HEAP_OPTS="-Xms1g -Xmx1g" \
      nohup kafka-server-start "$conf" > "target/kafka-$n.log" 2>&1 &
    echo $! > "target/kafka-$n.pid"
  done

  # Wait until every broker has registered, then create the topic.
  local ok=""
  for _ in $(seq 90); do
    if kafka-topics --bootstrap-server $BOOT --create --if-not-exists --topic events \
        --partitions "$PARTITIONS" --replication-factor "$RF" \
        --config min.insync.replicas="$MIN_ISR" > /dev/null 2>&1; then
      ok=1
      break
    fi
    sleep 1
  done
  if [ -z "$ok" ]; then
    echo "could not create the topic; see target/kafka-*.log" >&2
    exit 1
  fi

  printf '{"brokers": %s, "partitions": %s, "replication_factor": %s, "min_insync_replicas": %s, "jmx_ports": [%s]}\n' \
    "$BROKERS" "$PARTITIONS" "$RF" "$MIN_ISR" "$jmx_ports" > target/kafka-cluster.json
  echo "kafka up: $SHAPE (pids $(cat target/kafka-*.pid | tr '\n' ' '))"
}

stop() {
  kafka-server-stop > /dev/null 2>&1 || true
  local _
  for _ in $(seq 30); do
    pgrep -f kafka.Kafka > /dev/null || break
    sleep 1
  done
  rm -f target/kafka-*.pid target/kafka-cluster.json
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  *)
    echo "usage: $0 start|stop" >&2
    exit 2
    ;;
esac
