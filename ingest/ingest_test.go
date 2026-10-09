package main

import (
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func newAPI(t *testing.T) (*API, string) {
	t.Helper()
	dir := t.TempDir()
	spool, err := NewSpool(dir)
	if err != nil {
		t.Fatal(err)
	}
	return &API{spool: spool, maxValues: 5, maxBody: 1024, log: slog.New(slog.NewTextHandler(io.Discard, nil))}, dir
}

func do(h http.Handler, method, path, body string) *httptest.ResponseRecorder {
	w := httptest.NewRecorder()
	h.ServeHTTP(w, httptest.NewRequest(method, path, strings.NewReader(body)))
	return w
}

func TestCreateBatchWritesToInbox(t *testing.T) {
	api, dir := newAPI(t)
	h := api.Routes()

	w := do(h, "POST", "/api/v1/batches", `{"name":"sensor-1","values":[1.5,2,3]}`)
	if w.Code != http.StatusAccepted {
		t.Fatalf("status %d: %s", w.Code, w.Body)
	}
	var resp struct {
		ID        string `json:"id"`
		Status    string `json:"status"`
		StatusURL string `json:"status_url"`
	}
	_ = json.Unmarshal(w.Body.Bytes(), &resp)

	data, err := os.ReadFile(filepath.Join(dir, DirInbox, resp.ID+".json"))
	if err != nil {
		t.Fatalf("файл в inbox не создан: %v", err)
	}
	var b Batch
	if err := json.Unmarshal(data, &b); err != nil {
		t.Fatal(err)
	}
	if b.Name != "sensor-1" || len(b.Values) != 3 || b.ReceivedAt.IsZero() {
		t.Fatalf("batch %+v", b)
	}
	// Временных файлов после записи не остаётся.
	entries, _ := os.ReadDir(filepath.Join(dir, DirInbox))
	if len(entries) != 1 {
		t.Fatalf("в inbox %d файлов, ожидался 1", len(entries))
	}
	if w.Header().Get("Location") != "/api/v1/batches/"+resp.ID {
		t.Fatalf("Location %q", w.Header().Get("Location"))
	}
}

func TestCreateBatchValidation(t *testing.T) {
	api, _ := newAPI(t)
	h := api.Routes()
	cases := []struct {
		body   string
		status int
	}{
		{`{"name":"","values":[1]}`, 422},
		{`{"name":"плохое имя","values":[1]}`, 422},
		{`{"name":"ok","values":[]}`, 422},
		{`{"name":"ok","values":[1,2,3,4,5,6]}`, 422}, // больше maxValues
		{`{"name":"ok","values":["x"]}`, 400},
		{`{"name":"ok","values":[1],"extra":1}`, 400},
		{`not json`, 400},
		{`{"name":"ok","values":[` + strings.Repeat("1,", 600) + `1]}`, 413},
	}
	for _, c := range cases {
		if w := do(h, "POST", "/api/v1/batches", c.body); w.Code != c.status {
			t.Errorf("%.40s: status %d, want %d (%s)", c.body, w.Code, c.status, w.Body)
		}
	}
}

func TestBatchStatus(t *testing.T) {
	api, dir := newAPI(t)
	h := api.Routes()
	id := "0b5f1d3e-7c2a-4f6e-9a1b-2c3d4e5f6a7b"

	if w := do(h, "GET", "/api/v1/batches/"+id, ""); w.Code != 404 {
		t.Fatalf("неизвестный: %d", w.Code)
	}
	if w := do(h, "GET", "/api/v1/batches/not-uuid", ""); w.Code != 400 {
		t.Fatalf("не UUID: %d", w.Code)
	}
	for _, step := range []struct {
		dir  string
		want Status
	}{
		{DirInbox, StatusQueued},
		{DirProcessing, StatusProcessing},
		{DirOutbox, StatusDone},
	} {
		if err := os.WriteFile(filepath.Join(dir, step.dir, id+".json"), []byte("{}"), 0o644); err != nil {
			t.Fatal(err)
		}
		w := do(h, "GET", "/api/v1/batches/"+id, "")
		if !strings.Contains(w.Body.String(), string(step.want)) {
			t.Fatalf("в %s: %s", step.dir, w.Body)
		}
	}
}

func TestHealth(t *testing.T) {
	api, dir := newAPI(t)
	h := api.Routes()
	if w := do(h, "GET", "/health/ready", ""); w.Code != 200 {
		t.Fatalf("ready: %d", w.Code)
	}
	// Каталог inbox пропал (volume отмонтирован) — сервис не готов.
	if err := os.RemoveAll(filepath.Join(dir, DirInbox)); err != nil {
		t.Fatal(err)
	}
	if w := do(h, "GET", "/health/ready", ""); w.Code != 503 {
		t.Fatalf("ready без volume: %d", w.Code)
	}
	if w := do(h, "GET", "/health/live", ""); w.Code != 200 {
		t.Fatalf("live: %d", w.Code)
	}
}
