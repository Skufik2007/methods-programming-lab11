//! processor — Rust-сервис обработки пакетов.
//!
//! Цикл: забрать пакет из /data/inbox → посчитать статистику → записать отчёт
//! в /data/outbox. Рядом в отдельном потоке работает HTTP-сервер для
//! healthcheck (/health) и метрик (/metrics).
//!
//! `processor healthcheck` — проверка для Docker HEALTHCHECK: в образе scratch
//! нет curl, поэтому бинарь сам делает запрос к своему /health.

mod spool;
mod stats;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::json;

use spool::{Batch, Claimed, Spool};

#[derive(Default)]
struct Metrics {
    processed: AtomicU64,
    failed: AtomicU64,
    values: AtomicU64,
    /// Время последнего прохода цикла (секунды Unix): если цикл завис, /health отдаёт 503.
    heartbeat: AtomicU64,
}

#[derive(Serialize)]
struct Report<'a> {
    id: &'a str,
    name: &'a str,
    received_at: &'a str,
    processed_at: String,
    duration_ms: f64,
    processor: &'static str,
    stats: stats::Stats,
    histogram: stats::Histogram,
}

fn main() {
    let health_addr = env("HEALTH_ADDR", "0.0.0.0:9090");
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(probe(&health_addr));
    }

    let data_dir = env("DATA_DIR", "/data");
    let poll = Duration::from_millis(env("POLL_INTERVAL_MS", "500").parse().unwrap_or_else(|_| {
        log(
            "error",
            "POLL_INTERVAL_MS: ожидается целое число миллисекунд",
            json!({}),
        );
        std::process::exit(2);
    }));

    let spool = match Spool::open(&data_dir) {
        Ok(s) => s,
        Err(e) => {
            log(
                "error",
                "не удалось открыть каталог данных",
                json!({"data_dir": data_dir, "error": e.to_string()}),
            );
            std::process::exit(1);
        }
    };
    // Пакет, который пролежал в processing дольше этого времени, считается брошенным
    // (владелец упал или не смог записать отчёт) и возвращается в очередь.
    let stale_after = Duration::from_secs(
        env("PROCESSING_TIMEOUT_SEC", "300")
            .parse()
            .unwrap_or_else(|_| {
                log(
                    "error",
                    "PROCESSING_TIMEOUT_SEC: ожидается целое число секунд",
                    json!({}),
                );
                std::process::exit(2);
            }),
    );
    recover(&spool, stale_after);
    let mut last_recover = Instant::now();

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        // SIGTERM от docker stop: доделываем текущий пакет и выходим.
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .expect("обработчик сигналов");
    }

    let metrics = Arc::new(Metrics::default());
    metrics.heartbeat.store(now_unix(), Ordering::Relaxed);
    let server = match tiny_http::Server::http(&health_addr) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            log(
                "error",
                "не удалось открыть порт health",
                json!({"addr": health_addr, "error": e.to_string()}),
            );
            std::process::exit(1);
        }
    };
    let http_thread = {
        let (server, metrics, inbox) = (server.clone(), metrics.clone(), spool.dir(spool::INBOX));
        std::thread::spawn(move || serve_http(&server, &metrics, &inbox))
    };

    log(
        "info",
        "processor запущен",
        json!({"data_dir": data_dir, "poll_ms": poll.as_millis(), "health": health_addr}),
    );
    while !stop.load(Ordering::SeqCst) {
        metrics.heartbeat.store(now_unix(), Ordering::Relaxed);
        if last_recover.elapsed() >= RECOVER_INTERVAL {
            recover(&spool, stale_after);
            last_recover = Instant::now();
        }
        match spool.claim_next() {
            Ok(Some(claimed)) => handle(&spool, &claimed, &metrics),
            Ok(None) => sleep_interruptible(poll, &stop),
            Err(e) => {
                log("error", "чтение inbox", json!({"error": e.to_string()}));
                sleep_interruptible(poll, &stop);
            }
        }
    }

    log("info", "получен сигнал, останавливаемся", json!({}));
    server.unblock();
    let _ = http_thread.join();
    log(
        "info",
        "processor остановлен",
        json!({"processed": metrics.processed.load(Ordering::Relaxed)}),
    );
}

/// Как часто искать брошенные пакеты в processing.
const RECOVER_INTERVAL: Duration = Duration::from_secs(60);

fn recover(spool: &Spool, stale_after: Duration) {
    match spool.recover_stale(stale_after) {
        Ok(0) => {}
        Ok(n) => log(
            "warn",
            "в очередь возвращены пакеты, зависшие в processing",
            json!({"count": n, "stale_after_sec": stale_after.as_secs()}),
        ),
        Err(e) => log("error", "recover", json!({"error": e.to_string()})),
    }
}

fn handle(spool: &Spool, claimed: &Claimed, metrics: &Metrics) {
    let started = Instant::now();
    let result = std::fs::read(&claimed.path)
        .map_err(|e| format!("чтение файла: {e}"))
        .and_then(|data| {
            serde_json::from_slice::<Batch>(&data).map_err(|e| format!("некорректный JSON: {e}"))
        })
        .and_then(|batch| {
            // Имя файла задаёт ingest по id пакета; расхождение значит, что файл подменён или испорчен.
            if batch.id != claimed.id {
                return Err(format!(
                    "id в файле ({}) не совпадает с именем файла",
                    batch.id
                ));
            }
            let (stats, histogram) = stats::compute(&batch.values).map_err(|e| e.to_string())?;
            Ok((batch, stats, histogram))
        });

    match result {
        Ok((batch, stats, histogram)) => {
            let report = Report {
                id: &claimed.id,
                name: &batch.name,
                received_at: &batch.received_at,
                processed_at: rfc3339(SystemTime::now()),
                duration_ms: started.elapsed().as_secs_f64() * 1000.0,
                processor: concat!("rust-processor/", env!("CARGO_PKG_VERSION")),
                stats,
                histogram,
            };
            if let Err(e) = spool.complete(claimed, &report) {
                log(
                    "error",
                    "запись отчёта",
                    json!({"id": claimed.id, "error": e.to_string()}),
                );
                return;
            }
            metrics.processed.fetch_add(1, Ordering::Relaxed);
            metrics
                .values
                .fetch_add(batch.values.len() as u64, Ordering::Relaxed);
            log(
                "info",
                "пакет обработан",
                json!({"id": claimed.id, "values": batch.values.len(), "duration_ms": report.duration_ms}),
            );
        }
        Err(reason) => {
            metrics.failed.fetch_add(1, Ordering::Relaxed);
            log(
                "warn",
                "пакет отклонён",
                json!({"id": claimed.id, "error": reason}),
            );
            let info =
                json!({"id": claimed.id, "error": reason, "failed_at": rfc3339(SystemTime::now())});
            if let Err(e) = spool.fail(claimed, &info) {
                log(
                    "error",
                    "запись в failed",
                    json!({"id": claimed.id, "error": e.to_string()}),
                );
            }
        }
    }
}

fn serve_http(server: &tiny_http::Server, metrics: &Metrics, inbox: &std::path::Path) {
    for request in server.incoming_requests() {
        let (status, body) = match request.url() {
            "/health" => {
                let stale =
                    now_unix().saturating_sub(metrics.heartbeat.load(Ordering::Relaxed)) > 30;
                if stale || !inbox.is_dir() {
                    (
                        503,
                        json!({"status": "unhealthy", "loop_stale": stale, "inbox": inbox.is_dir()}),
                    )
                } else {
                    (200, json!({"status": "ok"}))
                }
            }
            "/metrics" => (
                200,
                json!({
                    "processed": metrics.processed.load(Ordering::Relaxed),
                    "failed": metrics.failed.load(Ordering::Relaxed),
                    "values": metrics.values.load(Ordering::Relaxed),
                }),
            ),
            _ => (404, json!({"error": "not found"})),
        };
        let header = tiny_http::Header::from_bytes("Content-Type", "application/json")
            .expect("корректный заголовок");
        let response = tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(header);
        let _ = request.respond(response);
    }
}

/// Спит interval, но просыпается раньше, если пришёл сигнал остановки.
fn sleep_interruptible(interval: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(50);
    let deadline = Instant::now() + interval;
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(step.min(deadline - Instant::now()));
    }
}

/// HTTP GET /health к самому себе через TcpStream — без зависимостей от HTTP-клиента.
fn probe(addr: &str) -> i32 {
    let port = addr.rsplit(':').next().unwrap_or("9090");
    let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{port}")) else {
        return 1;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream
        .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_err()
    {
        return 1;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    if response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200") {
        0
    } else {
        1
    }
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Одна JSON-строка на событие — тот же формат логов, что у Go-сервиса.
fn log(level: &str, msg: &str, fields: serde_json::Value) {
    let mut record = json!({"time": rfc3339(SystemTime::now()), "level": level, "msg": msg});
    if let (Some(obj), serde_json::Value::Object(extra)) = (record.as_object_mut(), fields) {
        obj.extend(extra);
    }
    println!("{record}");
}

/// RFC 3339 в UTC без внешних зависимостей (алгоритм days-from-civil Говарда Хиннанта).
fn rfc3339(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_dates() {
        let at = |s: u64, ms: u32| {
            UNIX_EPOCH + Duration::from_secs(s) + Duration::from_millis(ms.into())
        };
        assert_eq!(rfc3339(at(0, 0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(at(951_782_400, 5)), "2000-02-29T00:00:00.005Z"); // високосный день
        assert_eq!(rfc3339(at(1_791_071_999, 999)), "2026-10-03T23:59:59.999Z");
    }

    #[test]
    fn handle_writes_report_and_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path()).unwrap();
        let metrics = Metrics::default();
        std::fs::write(
            spool.dir(spool::INBOX).join("good.json"),
            r#"{"id":"good","name":"s1","values":[1,2,3,4],"received_at":"2026-10-09T10:00:00Z"}"#,
        )
        .unwrap();
        std::fs::write(spool.dir(spool::INBOX).join("bad.json"), "not json").unwrap();

        while let Some(c) = spool.claim_next().unwrap() {
            handle(&spool, &c, &metrics);
        }
        let report: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(spool.dir(spool::OUTBOX).join("good.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["stats"]["mean"], 2.5);
        assert_eq!(report["name"], "s1");
        assert!(spool.dir(spool::FAILED).join("bad.json").exists());
        assert_eq!(metrics.processed.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.failed.load(Ordering::Relaxed), 1);
    }
}
