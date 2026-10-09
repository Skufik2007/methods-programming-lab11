package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// Каталоги общего volume. Тот же контракт используют processor (Rust) и reports (Python),
// он описан в README, раздел «Обмен данными через volume».
const (
	DirInbox      = "inbox"      // новые пакеты от ingest
	DirProcessing = "processing" // пакет, взятый processor в работу
	DirOutbox     = "outbox"     // готовые отчёты
	DirFailed     = "failed"     // пакеты, которые не удалось обработать
)

// Batch — пакет измерений, как он лежит в inbox.
type Batch struct {
	ID         string    `json:"id"`
	Name       string    `json:"name"`
	Values     []float64 `json:"values"`
	ReceivedAt time.Time `json:"received_at"`
}

// Spool — очередь файлов на общем volume.
type Spool struct {
	root string
}

// NewSpool создаёт подкаталоги, если их ещё нет (volume может быть пустым).
func NewSpool(root string) (*Spool, error) {
	for _, d := range []string{DirInbox, DirProcessing, DirOutbox, DirFailed} {
		if err := os.MkdirAll(filepath.Join(root, d), 0o755); err != nil {
			return nil, fmt.Errorf("каталог %s: %w", d, err)
		}
	}
	return &Spool{root: root}, nil
}

// Enqueue атомарно кладёт пакет в inbox: сначала пишет во временный файл
// (имя с точкой — processor такие пропускает), затем переименовывает.
// rename в пределах одной файловой системы атомарен, поэтому processor
// никогда не увидит недописанный файл.
func (s *Spool) Enqueue(b Batch) error {
	data, err := json.Marshal(b)
	if err != nil {
		return err
	}
	dir := filepath.Join(s.root, DirInbox)
	tmp, err := os.CreateTemp(dir, ".incoming-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name()) // после успешного rename файла уже нет — ошибка игнорируется

	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		return err
	}
	// fsync до rename: иначе при сбое питания можно получить пустой файл под верным именем.
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), filepath.Join(dir, b.ID+".json"))
}

// Status — где сейчас пакет.
type Status string

const (
	StatusQueued     Status = "queued"
	StatusProcessing Status = "processing"
	StatusDone       Status = "done"
	StatusFailed     Status = "failed"
)

var ErrUnknownBatch = errors.New("пакет не найден")

// Status ищет пакет по каталогам. Порядок важен: файл переходит
// inbox -> processing -> outbox|failed, поэтому проверка идёт с конца,
// чтобы не пропустить пакет, переехавший между двумя проверками.
func (s *Spool) Status(id string) (Status, error) {
	for _, c := range []struct {
		dir    string
		status Status
	}{
		{DirOutbox, StatusDone},
		{DirFailed, StatusFailed},
		{DirProcessing, StatusProcessing},
		{DirInbox, StatusQueued},
	} {
		_, err := os.Stat(filepath.Join(s.root, c.dir, id+".json"))
		if err == nil {
			return c.status, nil
		}
		if !errors.Is(err, os.ErrNotExist) {
			return "", err
		}
	}
	return "", ErrUnknownBatch
}

// Writable проверяет, что в inbox можно писать (для /health/ready).
func (s *Spool) Writable() error {
	f, err := os.CreateTemp(filepath.Join(s.root, DirInbox), ".probe-*.tmp")
	if err != nil {
		return err
	}
	f.Close()
	return os.Remove(f.Name())
}
