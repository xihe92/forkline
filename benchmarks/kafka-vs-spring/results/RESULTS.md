## 1 broker(s), 6 partitions, 1 replica(s), linger.ms = 5  (results/run-20261120-235329-linger5.json)

Apple M5, 10 cores, 16 GB, macOS 27.2.1; Spring on openjdk version "25.0.5.1" 2026-11-17

| Target | Startup (s) | Idle RSS (MB) | RSS after run (MB) |
|---|---:|---:|---:|
| Forkline | 0.31 | 7.0 | 42.2 |
| Spring MVC | 1.22 | 223.1 | 328.9 |
| Spring MVC + virtual threads | 0.91 | 226.8 | 1074.3 |

### rate-1k: 1,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 997 | 5.856 | 7.32 | 9.558 | 0.16 | 164.7 | 18.3 | – | yes |
| Spring MVC | 996 | 3.853 | 7.062 | 10.182 | 0.23 | 229.0 | 238.6 | – | yes |
| Spring MVC + virtual threads | 996 | 3.461 | 6.827 | 8.945 | 0.23 | 235.0 | 230.6 | – | yes |

### rate-5k: 5,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,979 | 4.063 | 7.266 | 10.167 | 0.52 | 105.3 | 18.6 | – | yes |
| Spring MVC | 4,979 | 4.283 | 7.673 | 9.861 | 0.78 | 156.5 | 241.9 | – | yes |
| Spring MVC + virtual threads | 4,980 | 4.469 | 8.096 | 9.938 | 0.74 | 148.2 | 219.5 | – | yes |

### rate-10k: 10,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,956 | 4.27 | 8.542 | 13.278 | 0.81 | 81.6 | 20.5 | – | yes |
| Spring MVC | 9,953 | 4.615 | 9.011 | 12.178 | 1.42 | 142.5 | 282.6 | – | yes |
| Spring MVC + virtual threads | 9,959 | 5.075 | 9.069 | 11.279 | 1.21 | 121.1 | 230.8 | – | yes |

### rate-20k: 20,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,888 | 4.243 | 9.265 | 14.272 | 1.23 | 61.8 | 21.6 | – | yes |
| Spring MVC | 19,896 | 4.729 | 8.79 | 11.191 | 1.83 | 92.1 | 306.5 | – | yes |
| Spring MVC + virtual threads | 19,891 | 5.681 | 10.804 | 12.97 | 1.69 | 85.0 | 321.2 | – | yes |

### conns-64: 64 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,140 | 6.939 | 9.144 | 13.308 | 0.65 | 71.1 | 21.6 | – | yes |
| Spring MVC | 9,253 | 6.897 | 9.01 | 10.461 | 1.39 | 150.6 | 303.8 | – | yes |
| Spring MVC + virtual threads | 8,382 | 7.405 | 9.429 | 11.513 | 0.87 | 103.5 | 356.0 | – | yes |

### conns-256: 256 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 35,639 | 7.159 | 9.861 | 12.965 | 1.29 | 36.1 | 21.9 | – | yes |
| Spring MVC | 34,387 | 6.406 | 12.448 | 14.363 | 1.95 | 56.7 | 287.2 | – | yes |
| Spring MVC + virtual threads | 34,739 | 7.315 | 9.511 | 11.672 | 1.77 | 51.0 | 583.6 | – | yes |

### conns-1024: 1024 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 103,876 | 8.743 | 25.376 | 42.16 | 3.05 | 29.4 | 60.1 | – | yes |
| Spring MVC | 34,164 | 29.323 | 36.097 | 39.537 | 2.06 | 60.2 | 354.8 | – | yes |
| Spring MVC + virtual threads | 106,877 | 7.082 | 31.424 | 45.514 | 5.55 | 51.9 | 1543.5 | – | yes |

## 1 broker(s), 6 partitions, 1 replica(s), linger.ms = 0  (results/run-20261121-001019-linger0.json)

Apple M5, 10 cores, 16 GB, macOS 27.2.1; Spring on openjdk version "25.0.5.1" 2026-11-17

| Target | Startup (s) | Idle RSS (MB) | RSS after run (MB) |
|---|---:|---:|---:|
| Forkline | 0.06 | 6.9 | 43.0 |
| Spring MVC | 1.09 | 223.5 | 419.8 |
| Spring MVC + virtual threads | 1.04 | 203.2 | 1002.9 |

### rate-1k: 1,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 996 | 0.43 | 2.702 | 4.848 | 0.2 | 202.3 | 19.2 | – | yes |
| Spring MVC | 996 | 0.475 | 2.526 | 4.383 | 0.25 | 247.3 | 208.8 | – | yes |
| Spring MVC + virtual threads | 996 | 0.501 | 2.766 | 5.405 | 0.28 | 278.7 | 259.5 | – | yes |

### rate-5k: 5,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,982 | 0.644 | 6.208 | 10.35 | 0.71 | 142.9 | 19.8 | – | yes |
| Spring MVC | 4,979 | 0.687 | 5.941 | 9.717 | 0.89 | 178.6 | 214.5 | – | yes |
| Spring MVC + virtual threads | 4,978 | 0.794 | 5.453 | 9.187 | 0.89 | 178.0 | 271.2 | – | yes |

### rate-10k: 10,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,952 | 0.72 | 7.238 | 11.418 | 0.98 | 98.5 | 20.7 | – | yes |
| Spring MVC | 9,955 | 0.766 | 7.73 | 12.348 | 1.4 | 140.8 | 235.0 | – | yes |
| Spring MVC + virtual threads | 9,953 | 0.806 | 6.669 | 10.281 | 1.21 | 121.7 | 277.7 | – | yes |

### rate-20k: 20,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,901 | 0.78 | 7.46 | 12.094 | 1.27 | 63.9 | 22.5 | – | yes |
| Spring MVC | 19,910 | 0.859 | 7.021 | 11.886 | 1.88 | 94.5 | 238.5 | – | yes |
| Spring MVC + virtual threads | 19,903 | 0.871 | 7.629 | 11.961 | 1.58 | 79.3 | 287.3 | – | yes |

### conns-64: 64 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 92,601 | 0.64 | 1.199 | 7.232 | 3.29 | 35.6 | 19.6 | – | yes |
| Spring MVC | 73,205 | 0.779 | 2.06 | 7.924 | 4.54 | 62.0 | 355.2 | – | yes |
| Spring MVC + virtual threads | 97,187 | 0.621 | 1.256 | 6.866 | 4.83 | 49.7 | 274.0 | – | yes |

### conns-256: 256 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 100,022 | 2.363 | 5.947 | 12.966 | 3.12 | 31.2 | 21.4 | – | yes |
| Spring MVC | 104,864 | 2.191 | 6.217 | 13.174 | 5.33 | 50.8 | 365.3 | – | yes |
| Spring MVC + virtual threads | 124,919 | 1.857 | 5.545 | 12.451 | 5.52 | 44.2 | 687.4 | – | yes |

### conns-1024: 1024 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 102,372 | 8.651 | 24.487 | 29.343 | 3.36 | 32.9 | 60.3 | – | yes |
| Spring MVC | 119,102 | 7.999 | 17.432 | 25.311 | 5.51 | 46.3 | 449.6 | – | yes |
| Spring MVC + virtual threads | 115,376 | 4.916 | 80.735 | 128.777 | 5.86 | 50.8 | 1237.2 | – | yes |

## 3 broker(s), 10 partitions, 3 replica(s), linger.ms = 5  (results/run-20261121-111708-b3p10rf3-linger5.json)

Apple M5, 10 cores, 16 GB, macOS 27.2.1; Spring on openjdk version "25.0.5.1" 2026-11-17

| Target | Startup (s) | Idle RSS (MB) | RSS after run (MB) |
|---|---:|---:|---:|
| Forkline | 0.58 | 7.4 | 51.8 |
| Spring MVC | 1.14 | 218.9 | 248.7 |
| Spring MVC + virtual threads | 1.12 | 234.7 | 1042.8 |

### rate-1k: 1,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 996 | 6.287 | 18.191 | 24.72 | 0.17 | 175.0 | 17.3 | 1.5 | yes |
| Spring MVC | 996 | 5.054 | 16.337 | 23.018 | 0.21 | 211.3 | 296.9 | 2.5 | yes |
| Spring MVC + virtual threads | 997 | 5.138 | 17.113 | 23.997 | 0.26 | 260.7 | 273.8 | 2.5 | yes |

### rate-5k: 5,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,981 | 4.867 | 19.594 | 27.057 | 0.47 | 93.9 | 18.6 | 3.8 | yes |
| Spring MVC | 4,982 | 4.941 | 20.902 | 26.576 | 0.83 | 166.7 | 242.4 | 9.6 | yes |
| Spring MVC + virtual threads | 4,979 | 4.836 | 21.172 | 27.516 | 0.79 | 159.3 | 276.8 | 9.8 | yes |

### rate-10k: 10,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,956 | 4.473 | 19.747 | 26.444 | 0.67 | 67.8 | 20.4 | 6.7 | yes |
| Spring MVC | 9,952 | 5.712 | 21.956 | 27.287 | 1.36 | 136.9 | 218.5 | 20.6 | yes |
| Spring MVC + virtual threads | 9,951 | 5.4 | 22.086 | 29.63 | 1.1 | 110.9 | 267.7 | 21.0 | yes |

### rate-20k: 20,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,899 | 4.37 | 19.608 | 25.946 | 1.0 | 50.2 | 20.7 | 12.3 | yes |
| Spring MVC | 19,899 | 5.213 | 21.9 | 28.461 | 1.7 | 85.7 | 217.4 | 42.5 | yes |
| Spring MVC + virtual threads | 19,892 | 5.912 | 23.929 | 61.764 | 1.47 | 73.9 | 291.8 | 44.3 | yes |

### conns-64: 64 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 8,997 | 6.905 | 18.497 | 28.487 | 0.6 | 66.9 | 18.0 | 6.4 | yes |
| Spring MVC | 7,959 | 7.913 | 16.847 | 27.12 | 1.1 | 138.0 | 220.6 | 21.3 | yes |
| Spring MVC + virtual threads | 7,719 | 8.045 | 18.55 | 26.825 | 0.77 | 99.2 | 288.5 | 21.3 | yes |

### conns-256: 256 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 37,066 | 6.748 | 14.607 | 23.063 | 1.25 | 33.6 | 20.9 | 25.4 | yes |
| Spring MVC | 29,755 | 7.085 | 19.617 | 31.727 | 1.7 | 57.2 | 262.8 | 66.5 | yes |
| Spring MVC + virtual threads | 34,046 | 7.355 | 13.638 | 25.295 | 1.66 | 48.8 | 449.6 | 85.3 | yes |

### conns-1024: 1024 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 101,739 | 9.003 | 25.128 | 33.433 | 3.19 | 31.3 | 59.7 | 55.7 | yes |
| Spring MVC | 29,621 | 33.344 | 50.351 | 56.625 | 1.78 | 59.9 | 299.2 | 66.6 | NO {'connection error': 302} |
| Spring MVC + virtual threads | 101,185 | 7.455 | 35.829 | 62.206 | 5.33 | 52.7 | 1485.8 | 209.5 | yes |

## 3 broker(s), 10 partitions, 3 replica(s), linger.ms = 0  (results/run-20261121-113435-b3p10rf3-linger0.json)

Apple M5, 10 cores, 16 GB, macOS 27.2.1; Spring on openjdk version "25.0.5.1" 2026-11-17

| Target | Startup (s) | Idle RSS (MB) | RSS after run (MB) |
|---|---:|---:|---:|
| Forkline | 0.07 | 7.3 | 43.6 |
| Spring MVC | 1.08 | 213.8 | 289.2 |
| Spring MVC + virtual threads | 1.08 | 223.8 | 1019.5 |

### rate-1k: 1,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 996 | 0.807 | 13.354 | 19.366 | 0.19 | 195.0 | 17.2 | 1.0 | yes |
| Spring MVC | 996 | 0.783 | 14.035 | 21.299 | 0.22 | 224.3 | 188.7 | 1.1 | yes |
| Spring MVC + virtual threads | 996 | 0.77 | 13.556 | 22.894 | 0.25 | 255.3 | 180.4 | 1.1 | yes |

### rate-5k: 5,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,982 | 0.661 | 18.114 | 25.317 | 0.47 | 94.9 | 18.7 | 1.2 | yes |
| Spring MVC | 4,981 | 0.717 | 17.558 | 24.978 | 0.56 | 111.9 | 217.1 | 1.4 | yes |
| Spring MVC + virtual threads | 4,982 | 0.823 | 18.482 | 28.499 | 0.71 | 142.5 | 251.9 | 2.4 | yes |

### rate-10k: 10,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,963 | 0.709 | 16.241 | 24.758 | 0.66 | 66.6 | 20.6 | 1.4 | yes |
| Spring MVC | 9,962 | 0.782 | 17.271 | 37.994 | 0.77 | 77.1 | 214.6 | 1.4 | yes |
| Spring MVC + virtual threads | 9,961 | 0.754 | 17.886 | 27.576 | 0.87 | 87.0 | 253.7 | 3.0 | yes |

### rate-20k: 20,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,914 | 0.956 | 18.804 | 33.465 | 1.21 | 60.7 | 20.6 | 1.7 | yes |
| Spring MVC | 19,911 | 0.95 | 17.623 | 27.52 | 1.49 | 75.0 | 197.0 | 1.7 | yes |
| Spring MVC + virtual threads | 19,910 | 0.775 | 16.388 | 28.887 | 1.17 | 58.8 | 270.8 | 4.0 | yes |

### conns-64: 64 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 33,701 | 1.695 | 4.131 | 21.941 | 1.88 | 55.8 | 19.3 | 2.6 | yes |
| Spring MVC | 34,568 | 1.726 | 3.398 | 21.837 | 2.52 | 73.0 | 183.2 | 3.5 | yes |
| Spring MVC + virtual threads | 35,677 | 1.699 | 3.498 | 20.95 | 2.66 | 74.5 | 252.0 | 3.6 | yes |

### conns-256: 256 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 62,144 | 3.71 | 11.499 | 28.037 | 2.46 | 39.7 | 20.9 | 5.1 | yes |
| Spring MVC | 70,520 | 3.341 | 8.094 | 25.199 | 3.53 | 50.1 | 358.2 | 11.1 | yes |
| Spring MVC + virtual threads | 79,707 | 2.804 | 10.111 | 26.036 | 3.8 | 47.7 | 455.0 | 10.6 | yes |

### conns-1024: 1024 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 62,500 | 14.107 | 42.434 | 55.528 | 2.67 | 42.7 | 58.6 | 5.2 | yes |
| Spring MVC | 69,379 | 13.64 | 34.06 | 42.159 | 3.44 | 49.7 | 346.0 | 11.4 | yes |
| Spring MVC + virtual threads | 83,287 | 6.619 | 107.289 | 164.851 | 4.34 | 52.1 | 1283.7 | 12.9 | yes |


# Fire and forget (`--mode async`)

## 1 broker(s), 6 partitions, 1 replica(s), linger.ms = 5, fire and forget  (results/run-20261123-141056-b1p6rf1-linger5-async.json)

Apple M5, 10 cores, 16 GB, macOS 27.2.1; Spring on openjdk version "25.0.5.1" 2026-11-17

| Target | Startup (s) | Idle RSS (MB) | RSS after run (MB) |
|---|---:|---:|---:|
| Forkline | 0.06 | 7.2 | 45.1 |
| Forkline, 4 producers | 0.07 | 9.0 | 31.4 |
| Spring MVC | 1.08 | 233.5 | 410.0 |
| Spring MVC + virtual threads | 1.06 | 246.0 | 713.2 |

### rate-1k: 1,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 996 | 0.159 | 0.277 | 0.817 | 0.13 | 133.7 | 19.5 | 1.8 | yes |
| Forkline, 4 producers | 996 | 0.155 | 0.231 | 0.342 | 0.16 | 163.7 | 21.6 | 1.2 | yes |
| Spring MVC | 997 | 0.203 | 0.286 | 0.48 | 0.17 | 173.0 | 401.2 | 5.9 | yes |
| Spring MVC + virtual threads | 996 | 0.2 | 0.275 | 0.504 | 0.17 | 172.3 | 318.3 | 5.9 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 996 | 7 | 0.0 | 133.7 | 0.0 | 19.5 | – | – | 0.12 | 0 |
| Forkline, 4 producers | 996 | 9 | 0.0 | 163.7 | 0.0 | 21.6 | – | – | 0.12 | 0 |
| Spring MVC | 997 | 7 | 0.0 | 173.0 | 0.0 | 401.7 | – | 120 / – | 0.06 | 0 |
| Spring MVC + virtual threads | 996 | 7 | 0.0 | 172.0 | 0.01 | 318.4 | – | 55 / – | 0.05 | 0 |

### rate-5k: 5,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,979 | 0.228 | 0.491 | 3.715 | 0.42 | 85.3 | 19.6 | 5.7 | yes |
| Forkline, 4 producers | 4,980 | 0.221 | 0.474 | 1.222 | 0.54 | 108.8 | 21.7 | 2.2 | yes |
| Spring MVC | 4,979 | 0.307 | 0.558 | 2.089 | 0.65 | 129.9 | 404.3 | 31.1 | yes |
| Spring MVC + virtual threads | 4,979 | 0.279 | 0.536 | 2.112 | 0.63 | 125.8 | 322.1 | 31.0 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,979 | 27 | 0.0 | 85.3 | 0.0 | 19.5 | – | – | 0.14 | 0 |
| Forkline, 4 producers | 4,980 | 37 | 0.0 | 108.8 | 0.0 | 21.7 | – | – | 0.29 | 0 |
| Spring MVC | 4,979 | 40 | 0.0 | 129.9 | 0.0 | 404.5 | – | 116 / – | 0.07 | 0 |
| Spring MVC + virtual threads | 4,978 | 35 | 0.0 | 125.8 | 0.0 | 322.4 | – | 168 / – | 0.06 | 0 |

### rate-10k: 10,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,952 | 0.322 | 0.615 | 0.842 | 0.69 | 69.4 | 19.7 | 10.0 | yes |
| Forkline, 4 producers | 9,951 | 0.355 | 0.689 | 0.992 | 0.91 | 91.6 | 21.7 | 3.3 | yes |
| Spring MVC | 9,952 | 0.562 | 0.859 | 1.84 | 1.1 | 110.9 | 405.4 | 63.0 | yes |
| Spring MVC + virtual threads | 9,957 | 0.456 | 0.874 | 1.922 | 1.05 | 105.5 | 330.3 | 64.3 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,951 | 70 | 0.0 | 69.4 | 0.0 | 19.7 | – | – | 0.15 | 0 |
| Forkline, 4 producers | 9,951 | 60 | 0.0 | 91.6 | 0.0 | 21.7 | – | – | 0.41 | 0 |
| Spring MVC | 9,952 | 80 | 0.0 | 110.9 | 0.0 | 405.7 | – | 141 / – | 0.06 | 0 |
| Spring MVC + virtual threads | 9,957 | 80 | 0.0 | 105.5 | 0.0 | 330.2 | – | 179 / – | 0.06 | 0 |

### rate-20k: 20,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,894 | 0.475 | 1.187 | 2.777 | 1.03 | 52.0 | 19.8 | 20.4 | yes |
| Forkline, 4 producers | 19,900 | 0.443 | 1.258 | 3.945 | 1.2 | 60.4 | 21.8 | 5.7 | yes |
| Spring MVC | 19,901 | 0.639 | 1.428 | 3.59 | 1.62 | 81.5 | 407.8 | 134.7 | yes |
| Spring MVC + virtual threads | 19,900 | 0.681 | 1.508 | 2.686 | 1.64 | 82.4 | 332.0 | 135.1 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,893 | 126 | 0.0 | 52.0 | 0.0 | 18.3 | – | – | 0.14 | 0 |
| Forkline, 4 producers | 19,900 | 145 | 0.0 | 60.4 | 0.0 | 21.8 | – | – | 0.4 | 0 |
| Spring MVC | 19,900 | 140 | 0.0 | 81.5 | 0.0 | 407.9 | – | 95 / – | 0.06 | 0 |
| Spring MVC + virtual threads | 19,899 | 160 | 0.0 | 82.4 | 0.01 | 332.0 | – | 154 / – | 0.06 | 0 |

### overload-150k: 150,000 req/s offered, 1024 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 120,186 | 2878.118 | 5624.058 | 5672.508 | 3.1 | 25.8 | 46.9 | 105.2 | yes |
| Forkline, 4 producers | 148,184 | 1.119 | 5.469 | 19.969 | 4.33 | 29.2 | 49.2 | 34.0 | yes |
| Spring MVC | 148,091 | 1.25 | 29.538 | 38.065 | 4.82 | 32.5 | 625.2 | 928.6 | yes |
| Spring MVC + virtual threads | 148,118 | 1.157 | 6.958 | 23.135 | 5.07 | 34.2 | 612.4 | 906.3 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 120,184 | 533 | 0.0 | 25.8 | 0.0 | 32.4 | – | – | 0.08 | 0 |
| Forkline, 4 producers | 148,182 | 1,147 | 0.0 | 29.2 | 0.0 | 33.7 | – | – | 0.2 | 0 |
| Spring MVC | 148,089 | 1,666 | 0.0 | 32.5 | 0.0 | 391.8 | – | 288 / – | 0.04 | 0 |
| Spring MVC + virtual threads | 148,116 | 1,723 | 0.0 | 34.2 | 0.0 | 503.7 | – | 336 / – | 0.03 | 0 |

### conns-64: 64 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 118,648 | 0.526 | 0.971 | 1.179 | 3.35 | 28.3 | 33.1 | 104.0 | yes |
| Forkline, 4 producers | 172,613 | 0.347 | 0.768 | 1.715 | 4.54 | 26.3 | 34.8 | 38.6 | yes |
| Spring MVC | 141,981 | 0.378 | 1.564 | 3.426 | 4.94 | 34.8 | 393.5 | 788.3 | yes |
| Spring MVC + virtual threads | 161,756 | 0.36 | 1.004 | 2.76 | 5.41 | 33.5 | 506.3 | 863.7 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 118,646 | 507 | 0.0 | 28.3 | 0.0 | 31.2 | – | – | 0.08 | 0 |
| Forkline, 4 producers | 172,611 | 1,105 | 0.0 | 26.3 | 0.0 | 32.9 | – | – | 0.22 | 0 |
| Spring MVC | 141,979 | 1,530 | 0.0 | 34.8 | 0.0 | 385.6 | – | 106 / – | 0.04 | 0 |
| Spring MVC + virtual threads | 161,753 | 1,628 | 0.0 | 33.5 | 0.0 | 493.4 | – | 152 / – | 0.03 | 0 |

### conns-256: 256 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 118,479 | 2.078 | 4.503 | 5.61 | 3.29 | 27.8 | 35.0 | 104.3 | yes |
| Forkline, 4 producers | 171,156 | 1.381 | 3.512 | 6.058 | 4.58 | 26.8 | 37.1 | 38.5 | yes |
| Spring MVC | 162,819 | 1.414 | 4.731 | 15.816 | 5.34 | 32.8 | 525.3 | 971.3 | yes |
| Spring MVC + virtual threads | 163,800 | 1.426 | 3.607 | 10.878 | 5.4 | 33.0 | 538.6 | 915.7 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 118,477 | 666 | 0.0 | 27.8 | 0.0 | 27.7 | – | – | 0.08 | 0 |
| Forkline, 4 producers | 171,154 | 687 | 0.0 | 26.8 | 0.0 | 29.3 | – | – | 0.22 | 0 |
| Spring MVC | 162,815 | 4,856 | 0.0 | 32.8 | 0.0 | 387.6 | – | 179 / – | 0.04 | 0 |
| Spring MVC + virtual threads | 163,796 | 1,282 | 0.0 | 33.0 | 0.0 | 503.1 | – | 238 / – | 0.03 | 0 |

### conns-1024: 1024 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 117,884 | 6.483 | 25.884 | 32.232 | 3.36 | 28.5 | 46.3 | 103.5 | yes |
| Forkline, 4 producers | 168,164 | 4.943 | 20.699 | 32.525 | 4.69 | 27.9 | 47.8 | 38.0 | yes |
| Spring MVC | 170,019 | 5.571 | 21.29 | 48.795 | 5.46 | 32.1 | 463.8 | 1076.3 | yes |
| Spring MVC + virtual threads | 169,811 | 5.707 | 10.662 | 30.955 | 5.73 | 33.7 | 790.4 | 987.4 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 117,882 | 700 | 0.0 | 28.5 | 0.0 | 45.1 | – | – | 0.09 | 0 |
| Forkline, 4 producers | 168,162 | 825 | 0.0 | 27.9 | 0.0 | 31.4 | – | – | 0.23 | 0 |
| Spring MVC | 170,015 | 15,181 | 0.0 | 32.1 | 0.0 | 408.7 | – | 180 / – | 0.04 | 0 |
| Spring MVC + virtual threads | 169,808 | 2,657 | 0.0 | 33.7 | 0.0 | 711.7 | – | 573 / – | 0.04 | 0 |

## 3 broker(s), 10 partitions, 3 replica(s), linger.ms = 5, fire and forget  (results/run-20261123-143705-b3p10rf3-linger5-async.json)

Apple M5, 10 cores, 16 GB, macOS 27.2.1; Spring on openjdk version "25.0.5.1" 2026-11-17

| Target | Startup (s) | Idle RSS (MB) | RSS after run (MB) |
|---|---:|---:|---:|
| Forkline | 0.06 | 7.5 | 30.2 |
| Forkline, 4 producers | 0.06 | 10.3 | 33.5 |
| Spring MVC | 1.12 | 220.2 | 343.8 |
| Spring MVC + virtual threads | 1.04 | 247.9 | 523.2 |

### rate-1k: 1,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 997 | 0.152 | 0.263 | 0.515 | 0.13 | 132.0 | 20.0 | 1.5 | yes |
| Forkline, 4 producers | 996 | 0.16 | 0.474 | 2.226 | 0.17 | 175.3 | 17.9 | 1.1 | yes |
| Spring MVC | 996 | 0.21 | 0.359 | 0.697 | 0.19 | 188.3 | 301.0 | 2.6 | yes |
| Spring MVC + virtual threads | 996 | 0.208 | 0.354 | 0.645 | 0.19 | 192.7 | 275.9 | 2.6 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 997 | 21 | 0.0 | 132.0 | 0.0 | 18.9 | – | – | 0.58 | 0 |
| Forkline, 4 producers | 993 | 61 | 0.11 | 175.0 | 0.01 | 17.8 | – | – | 0.69 | 0 |
| Spring MVC | 996 | 10 | 0.0 | 188.3 | 0.0 | 294.3 | – | 112 / – | 0.46 | 0 |
| Spring MVC + virtual threads | 996 | 22 | 0.0 | 192.7 | 0.0 | 276.1 | – | 217 / – | 0.47 | 0 |

### rate-5k: 5,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,980 | 0.175 | 0.488 | 0.732 | 0.39 | 77.5 | 19.3 | 3.9 | yes |
| Forkline, 4 producers | 4,981 | 0.174 | 0.473 | 0.695 | 0.48 | 96.3 | 18.5 | 1.7 | yes |
| Spring MVC | 4,981 | 0.297 | 0.886 | 1.554 | 0.62 | 124.4 | 295.4 | 10.0 | yes |
| Spring MVC + virtual threads | 4,981 | 0.296 | 0.858 | 1.285 | 0.65 | 130.9 | 316.5 | 10.1 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 4,980 | 107 | 0.0 | 77.5 | 0.0 | 18.6 | – | – | 0.83 | 0 |
| Forkline, 4 producers | 4,981 | 121 | 0.0 | 96.3 | 0.0 | 17.5 | – | – | 1.33 | 0 |
| Spring MVC | 4,981 | 133 | 0.0 | 124.4 | 0.0 | 285.6 | – | 221 / – | 0.68 | 0 |
| Spring MVC + virtual threads | 4,981 | 95 | 0.0 | 130.9 | 0.0 | 312.0 | – | 162 / – | 0.7 | 0 |

### rate-10k: 10,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,959 | 0.229 | 0.636 | 0.917 | 0.59 | 59.4 | 18.5 | 6.8 | yes |
| Forkline, 4 producers | 9,962 | 0.186 | 0.379 | 0.559 | 0.64 | 64.0 | 18.8 | 2.4 | yes |
| Spring MVC | 9,954 | 0.434 | 1.274 | 1.779 | 1.01 | 101.7 | 286.9 | 21.3 | yes |
| Spring MVC + virtual threads | 9,958 | 0.4 | 1.213 | 4.028 | 0.92 | 92.8 | 318.9 | 21.0 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 9,958 | 181 | 0.0 | 59.4 | 0.0 | 18.3 | – | – | 0.85 | 0 |
| Forkline, 4 producers | 9,962 | 181 | 0.0 | 64.0 | 0.0 | 14.8 | – | – | 1.33 | 0 |
| Spring MVC | 9,954 | 249 | 0.0 | 101.7 | 0.0 | 267.9 | – | 198 / – | 0.65 | 0 |
| Spring MVC + virtual threads | 9,958 | 170 | 0.0 | 92.8 | 0.0 | 275.4 | – | 136 / – | 0.64 | 0 |

### rate-20k: 20,000 req/s offered, 256 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,904 | 0.327 | 0.998 | 1.721 | 0.89 | 44.9 | 18.6 | 12.6 | yes |
| Forkline, 4 producers | 19,912 | 0.227 | 0.447 | 0.779 | 0.95 | 47.8 | 18.9 | 3.9 | yes |
| Spring MVC | 19,904 | 0.502 | 1.7 | 3.043 | 1.38 | 69.2 | 271.9 | 42.9 | yes |
| Spring MVC + virtual threads | 19,899 | 0.531 | 1.951 | 4.323 | 1.34 | 67.1 | 275.5 | 44.0 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 19,903 | 346 | 0.0 | 44.9 | 0.0 | 18.4 | – | – | 0.83 | 0 |
| Forkline, 4 producers | 19,912 | 382 | 0.0 | 47.8 | 0.01 | 14.8 | – | – | 1.38 | 0 |
| Spring MVC | 19,904 | 425 | 0.0 | 69.2 | 0.0 | 268.5 | – | 130 / – | 0.53 | 0 |
| Spring MVC + virtual threads | 19,899 | 400 | 0.0 | 67.1 | 0.0 | 259.9 | – | 183 / – | 0.54 | 0 |

### overload-150k: 150,000 req/s offered, 1024 connections

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 105,203 | 4037.16 | 8626.764 | 8689.816 | 3.04 | 28.9 | 47.4 | 57.6 | yes |
| Forkline, 4 producers | 128,346 | 1645.709 | 3918.65 | 3945.702 | 4.12 | 32.1 | 47.7 | 19.2 | yes; oha lost 26 replies |
| Spring MVC | 148,093 | 1.517 | 146.739 | 161.585 | 4.72 | 31.9 | 650.7 | 311.8 | yes |
| Spring MVC + virtual threads | 148,044 | 1.469 | 113.002 | 130.92 | 4.98 | 33.7 | 546.5 | 298.6 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 105,202 | 1,956 | 0.0 | 28.9 | 0.0 | 35.8 | – | – | 0.73 | 0 |
| Forkline, 4 producers | 128,345 | 1,036 | 0.0 | 32.1 | 0.0 | 32.0 | – | – | 1.72 | 0 |
| Spring MVC | 148,090 | 2,900 | 0.0 | 31.9 | 0.01 | 563.8 | – | 342 / – | 0.36 | 0 |
| Spring MVC + virtual threads | 148,041 | 3,679 | 0.0 | 33.7 | 0.0 | 524.8 | – | 226 / – | 0.33 | 0 |

### conns-64: 64 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 109,418 | 0.566 | 1.104 | 1.439 | 3.25 | 29.7 | 36.6 | 59.6 | yes |
| Forkline, 4 producers | 130,566 | 0.421 | 1.509 | 5.71 | 3.73 | 28.5 | 23.7 | 19.0 | yes |
| Spring MVC | 136,746 | 0.394 | 1.679 | 3.754 | 4.72 | 34.5 | 526.7 | 253.0 | yes |
| Spring MVC + virtual threads | 153,274 | 0.366 | 1.167 | 3.594 | 5.06 | 33.0 | 482.2 | 279.0 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 109,417 | 1,885 | 0.0 | 29.7 | 0.0 | 12.4 | – | – | 0.68 | 0 |
| Forkline, 4 producers | 130,564 | 1,547 | 0.0 | 28.5 | 0.0 | 12.3 | – | – | 1.95 | 0 |
| Spring MVC | 136,743 | 3,612 | 0.0 | 34.5 | 0.0 | 490.5 | – | 465 / – | 0.4 | 0 |
| Spring MVC + virtual threads | 153,272 | 903 | 0.0 | 33.0 | 0.0 | 377.9 | – | 415 / – | 0.37 | 0 |

### conns-256: 256 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 108,149 | 2.264 | 5.09 | 6.722 | 3.2 | 29.6 | 20.3 | 59.3 | yes |
| Forkline, 4 producers | 131,068 | 1.682 | 6.034 | 18.063 | 3.79 | 28.9 | 20.3 | 19.3 | yes |
| Spring MVC | 156,783 | 1.472 | 5.135 | 18.533 | 5.1 | 32.5 | 520.2 | 315.7 | yes |
| Spring MVC + virtual threads | 156,887 | 1.48 | 3.981 | 12.122 | 5.13 | 32.7 | 609.7 | 299.7 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 108,147 | 2,293 | 0.0 | 29.6 | 0.0 | 17.3 | – | – | 0.68 | 0 |
| Forkline, 4 producers | 131,066 | 552 | 0.0 | 28.9 | 0.0 | 15.9 | – | – | 1.87 | 0 |
| Spring MVC | 156,780 | 2,815 | 0.0 | 32.5 | 0.0 | 463.9 | – | 290 / – | 0.38 | 0 |
| Spring MVC + virtual threads | 156,885 | 20,636 | 0.0 | 32.7 | 0.0 | 448.9 | – | 328 / – | 0.35 | 0 |

### conns-1024: 1024 connections, as fast as possible

| Target | req/s | p50 ms | p99 ms | p99.9 ms | CPU cores | CPU µs/req | Peak RSS MB | Records/produce req | All in Kafka |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 106,804 | 7.178 | 29.056 | 38.032 | 3.27 | 30.6 | 47.5 | 58.5 | yes |
| Forkline, 4 producers | 125,724 | 6.068 | 32.626 | 56.011 | 4.0 | 31.8 | 48.3 | 18.6 | yes; oha lost 7 replies |
| Spring MVC | 161,478 | 5.83 | 21.903 | 48.661 | 5.16 | 32.0 | 503.2 | 352.0 | yes |
| Spring MVC + virtual threads | 150,133 | 6.267 | 14.199 | 44.587 | 4.92 | 32.8 | 640.7 | 293.7 | yes |

Backlog and drain:

| Target | Acked/s | Peak backlog | Drain s | Accept CPU µs/req | Drain CPU s | RSS drained MB | RSS +30 s MB | Heap used drained / +30 s MB | Broker CPU cores | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Forkline | 106,802 | 1,554 | 0.0 | 30.6 | 0.0 | 32.0 | – | – | 0.68 | 0 |
| Forkline, 4 producers | 125,722 | 4,676 | 0.0 | 31.8 | 0.0 | 33.5 | – | – | 1.9 | 0 |
| Spring MVC | 161,474 | 7,887 | 0.0 | 32.0 | 0.0 | 367.6 | – | 410 / – | 0.37 | 0 |
| Spring MVC + virtual threads | 150,130 | 2,943 | 0.0 | 32.8 | 0.0 | 522.8 | – | 271 / – | 0.36 | 0 |

