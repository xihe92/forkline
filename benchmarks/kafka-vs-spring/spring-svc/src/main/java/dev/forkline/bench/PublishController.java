package dev.forkline.bench;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.LongAdder;

import org.apache.kafka.clients.producer.RecordMetadata;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.http.HttpStatus;
import org.springframework.kafka.core.KafkaTemplate;
import org.springframework.kafka.support.SendResult;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.server.ResponseStatusException;

import tools.jackson.databind.json.JsonMapper;

/**
 * {@code GET /publish?user_id=42&event=click&amount=9.99&currency=USD}: parse and
 * validate the query, publish the event to Kafka as JSON keyed by {@code user_id},
 * and reply. The same rules as the Forkline service's QueryDecoder.
 *
 * <p>With {@code bench.mode=wait} (the default) the reply waits for the broker's
 * acknowledgement and is {@code {"partition":..,"offset":..}}. With
 * {@code bench.mode=async} the record is handed to the Kafka client, the reply is
 * {@code {"accepted":true}} at once, and {@code whenComplete} counts the
 * acknowledgement later. {@code GET /stats} returns the counts.
 */
@RestController
class PublishController {

	private static final Logger log = LoggerFactory.getLogger(PublishController.class);

	/** The event published to Kafka (component names are the JSON field names). */
	record Event(long user_id, String event, double amount, String currency, long ts_ms) {
	}

	/** Where the broker stored the record (wait mode). */
	record Delivery(int partition, long offset) {
	}

	/** Handed to the Kafka client; the broker hasn't answered yet (async mode). */
	record Accepted(boolean accepted) {
	}

	/** The same fields as the Forkline service's /stats; the Java client has no queue count. */
	record Stats(String mode, long accepted, long acked, long failed, long refused, long pending,
			Long client_queue) {
	}

	private static final Accepted ACCEPTED = new Accepted(true);

	private final KafkaTemplate<String, byte[]> kafka;
	private final JsonMapper json;
	private final String topic;
	private final boolean async;
	private final LongAdder accepted = new LongAdder();
	private final LongAdder acked = new LongAdder();
	private final LongAdder failed = new LongAdder();
	private final LongAdder refused = new LongAdder();

	PublishController(KafkaTemplate<String, byte[]> kafka, JsonMapper json,
			@Value("${bench.topic:events}") String topic, @Value("${bench.mode:wait}") String mode) {
		this.kafka = kafka;
		this.json = json;
		this.topic = topic;
		this.async = switch (mode) {
			case "wait" -> false;
			case "async" -> true;
			default -> throw new IllegalArgumentException(
					"unknown bench.mode `" + mode + "` (expected wait or async)");
		};
	}

	@GetMapping("/publish")
	Object publish(@RequestParam("user_id") long userId, @RequestParam("event") String event,
			@RequestParam(name = "amount", defaultValue = "0") double amount,
			@RequestParam(name = "currency", defaultValue = "USD") String currency) throws Exception {
		if (userId < 0) {
			throw bad("user_id must be an unsigned integer");
		}
		if (event.isEmpty() || event.codePointCount(0, event.length()) > 64) {
			throw bad("event must be 1 to 64 characters");
		}
		if (!Double.isFinite(amount)) {
			throw bad("amount must be a number");
		}
		if (currency.codePointCount(0, currency.length()) != 3) {
			throw bad("currency must be a 3-letter code");
		}
		byte[] payload = json.writeValueAsBytes(new Event(userId, event, amount, currency, System.currentTimeMillis()));
		// Counted before the send, so the completion can never run ahead of it.
		accepted.increment();
		CompletableFuture<SendResult<String, byte[]>> sent;
		try {
			sent = kafka.send(topic, Long.toString(userId), payload);
		}
		catch (RuntimeException e) {
			if (!async) {
				failure(e);
				throw e;
			}
			accepted.decrement();
			refused.increment();
			throw new ResponseStatusException(HttpStatus.INTERNAL_SERVER_ERROR, "kafka refused the record: " + e);
		}
		if (async) {
			sent.whenComplete((result, error) -> {
				if (error == null) {
					acked.increment();
				}
				else {
					failure(error);
				}
			});
			return ACCEPTED;
		}
		RecordMetadata m;
		try {
			m = sent.get(2, TimeUnit.SECONDS).getRecordMetadata();
		}
		catch (Exception e) {
			failure(e);
			throw e;
		}
		acked.increment();
		return new Delivery(m.partition(), m.offset());
	}

	@GetMapping("/stats")
	Stats stats() {
		// Read the outcomes before `accepted`, so pending is never negative.
		long a = acked.sum();
		long f = failed.sum();
		long r = refused.sum();
		long acc = accepted.sum();
		return new Stats(async ? "async" : "wait", acc, a, f, r, Math.max(0, acc - a - f), null);
	}

	private void failure(Throwable error) {
		// Log the first few, then every 10,000th, so a broken broker can't
		// flood the log during a benchmark.
		failed.increment();
		long n = failed.sum();
		if (n <= 10 || n % 10_000 == 0) {
			log.warn("kafka delivery failed ({} so far): {}", n, error.toString());
		}
	}

	private static ResponseStatusException bad(String message) {
		return new ResponseStatusException(HttpStatus.BAD_REQUEST, message);
	}
}
