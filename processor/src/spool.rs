//! Работа с общим volume: забрать пакет из inbox, записать отчёт в outbox.
//!
//! Все переходы файлов — через rename, который атомарен в пределах одной файловой
//! системы. Поэтому ни один сервис не увидит файл наполовину записанным,
//! а два экземпляра processor не возьмут один пакет: rename удастся только одному.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

pub const INBOX: &str = "inbox";
pub const PROCESSING: &str = "processing";
pub const OUTBOX: &str = "outbox";
pub const FAILED: &str = "failed";

#[derive(Debug, Deserialize)]
pub struct Batch {
    pub id: String,
    pub name: String,
    pub values: Vec<f64>,
    pub received_at: String,
}

pub struct Spool {
    root: PathBuf,
}

/// Пакет, взятый в работу (лежит в processing/).
pub struct Claimed {
    pub id: String,
    pub path: PathBuf,
}

impl Spool {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        for dir in [INBOX, PROCESSING, OUTBOX, FAILED] {
            fs::create_dir_all(root.join(dir))?;
        }
        Ok(Spool { root })
    }

    pub fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Возвращает в inbox пакеты, которые лежат в processing дольше `stale_after`:
    /// их владелец упал или не смог записать отчёт.
    ///
    /// Свежие пакеты не трогаются — их прямо сейчас обрабатывает другой экземпляр
    /// processor. Время отсчитывается от захвата: claim_next обновляет mtime файла.
    pub fn recover_stale(&self, stale_after: Duration) -> io::Result<usize> {
        let now = SystemTime::now();
        let mut n = 0;
        for entry in fs::read_dir(self.dir(PROCESSING))? {
            let entry = entry?;
            if !is_batch_file(&entry.path()) {
                continue;
            }
            let claimed_at = entry.metadata()?.modified()?;
            if now.duration_since(claimed_at).unwrap_or_default() < stale_after {
                continue;
            }
            match fs::rename(entry.path(), self.dir(INBOX).join(entry.file_name())) {
                Ok(()) => n += 1,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {} // уже вернул другой экземпляр
                Err(e) => return Err(e),
            }
        }
        Ok(n)
    }

    /// Берёт самый старый пакет из inbox, переименовывая его в processing/.
    /// Если пакет успел забрать другой экземпляр, пробует следующий.
    pub fn claim_next(&self) -> io::Result<Option<Claimed>> {
        let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for entry in fs::read_dir(self.dir(INBOX))? {
            let entry = entry?;
            let path = entry.path();
            if is_batch_file(&path) {
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                candidates.push((modified, path));
            }
        }
        candidates.sort();

        for (_, path) in candidates {
            let file_name = path.file_name().expect("путь из read_dir").to_owned();
            let target = self.dir(PROCESSING).join(&file_name);
            match fs::rename(&path, &target) {
                Ok(()) => {
                    // rename сохраняет mtime создания файла; отмечаем момент захвата,
                    // чтобы recover_stale считал время обработки, а не время в очереди.
                    fs::File::options()
                        .write(true)
                        .open(&target)?
                        .set_modified(SystemTime::now())?;
                    let id = Path::new(&file_name)
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    return Ok(Some(Claimed { id, path: target }));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue, // забрал другой экземпляр
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    /// Пишет отчёт в outbox и удаляет пакет из processing.
    pub fn complete<T: Serialize>(&self, claimed: &Claimed, report: &T) -> io::Result<()> {
        write_atomic(&self.dir(OUTBOX), &claimed.id, report)?;
        remove_claimed(&claimed.path)
    }

    /// Пишет причину ошибки в failed/ и удаляет пакет из processing.
    pub fn fail<T: Serialize>(&self, claimed: &Claimed, info: &T) -> io::Result<()> {
        write_atomic(&self.dir(FAILED), &claimed.id, info)?;
        remove_claimed(&claimed.path)
    }
}

/// Пакета в processing может уже не быть: если обработка шла дольше тайм-аута,
/// recover_stale другого экземпляра вернул его в очередь. Результат тот же
/// (отчёт перезапишется идентичным), поэтому это не ошибка.
fn remove_claimed(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Пакеты — файлы *.json; временные файлы начинаются с точки и пропускаются.
fn is_batch_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    !name.starts_with('.') && name.ends_with(".json")
}

/// Запись через временный файл + fsync + rename: читатель видит либо старое, либо полное новое содержимое.
fn write_atomic<T: Serialize>(dir: &Path, id: &str, value: &T) -> io::Result<()> {
    let tmp = dir.join(format!(".{id}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        serde_json::to_writer(&mut f, value)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    fs::rename(&tmp, dir.join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(spool: &Spool, dir: &str, name: &str, body: &str) {
        fs::write(spool.dir(dir).join(name), body).unwrap();
    }

    #[test]
    fn claim_skips_temp_files_and_moves_to_processing() {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path()).unwrap();
        put(&spool, INBOX, ".incoming-1.tmp", "{}");
        put(&spool, INBOX, "abc.json", "{}");

        let claimed = spool.claim_next().unwrap().expect("пакет должен найтись");
        assert_eq!(claimed.id, "abc");
        assert!(spool.dir(PROCESSING).join("abc.json").exists());
        assert!(!spool.dir(INBOX).join("abc.json").exists());
        assert!(
            spool.claim_next().unwrap().is_none(),
            "временный файл не берётся"
        );
    }

    #[test]
    fn complete_and_fail_write_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path()).unwrap();
        put(&spool, INBOX, "a.json", "{}");
        put(&spool, INBOX, "b.json", "{}");

        let a = spool.claim_next().unwrap().unwrap();
        spool
            .complete(&a, &serde_json::json!({"ok": true}))
            .unwrap();
        let b = spool.claim_next().unwrap().unwrap();
        spool.fail(&b, &serde_json::json!({"error": "x"})).unwrap();

        let report = fs::read_to_string(spool.dir(OUTBOX).join(format!("{}.json", a.id))).unwrap();
        assert_eq!(report.trim(), r#"{"ok":true}"#);
        assert!(spool.dir(FAILED).join(format!("{}.json", b.id)).exists());
        assert_eq!(fs::read_dir(spool.dir(PROCESSING)).unwrap().count(), 0);
        // временные файлы не остаются
        for dir in [OUTBOX, FAILED] {
            assert!(fs::read_dir(spool.dir(dir)).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')));
        }
    }

    fn set_age(path: &Path, age: Duration) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    }

    #[test]
    fn recover_returns_only_stale_batches() {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path()).unwrap();
        put(&spool, PROCESSING, "stuck.json", "{}");
        put(&spool, PROCESSING, "busy.json", "{}");
        set_age(
            &spool.dir(PROCESSING).join("stuck.json"),
            Duration::from_secs(600),
        );

        assert_eq!(spool.recover_stale(Duration::from_secs(300)).unwrap(), 1);
        assert!(spool.dir(INBOX).join("stuck.json").exists());
        // Пакет, захваченный только что (другим экземпляром), остаётся у него.
        assert!(spool.dir(PROCESSING).join("busy.json").exists());
    }

    #[test]
    fn claim_marks_claim_time() {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path()).unwrap();
        put(&spool, INBOX, "old.json", "{}");
        // Пакет долго ждал в очереди — это не значит, что его обработка зависла.
        set_age(
            &spool.dir(INBOX).join("old.json"),
            Duration::from_secs(3600),
        );

        spool.claim_next().unwrap().unwrap();
        assert_eq!(spool.recover_stale(Duration::from_secs(300)).unwrap(), 0);
    }

    #[test]
    fn complete_tolerates_batch_reclaimed_by_other_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path()).unwrap();
        put(&spool, INBOX, "a.json", "{}");
        let a = spool.claim_next().unwrap().unwrap();
        // Обработка затянулась, другой экземпляр вернул пакет в очередь.
        set_age(&a.path, Duration::from_secs(600));
        spool.recover_stale(Duration::from_secs(300)).unwrap();

        spool
            .complete(&a, &serde_json::json!({"ok": true}))
            .unwrap();
        assert!(spool.dir(OUTBOX).join("a.json").exists());
    }
}
