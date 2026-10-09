package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"regexp"
	"time"

	"github.com/google/uuid"
)

var namePattern = regexp.MustCompile(`^[A-Za-z0-9_-]{1,64}$`)

type batchRequest struct {
	Name   string    `json:"name"`
	Values []float64 `json:"values"`
}

type API struct {
	spool     *Spool
	maxValues int
	maxBody   int64
	log       *slog.Logger
}

func (a *API) Routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /health/live", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusOK, map[string]string{"status": "ok"})
	})
	mux.HandleFunc("GET /health/ready", a.ready)
	mux.HandleFunc("POST /api/v1/batches", a.createBatch)
	mux.HandleFunc("GET /api/v1/batches/{id}", a.batchStatus)
	return a.logRequests(mux)
}

func (a *API) ready(w http.ResponseWriter, r *http.Request) {
	if err := a.spool.Writable(); err != nil {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"status": "volume_unavailable", "error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"status": "ready"})
}

func (a *API) createBatch(w http.ResponseWriter, r *http.Request) {
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, a.maxBody))
	dec.DisallowUnknownFields()
	var req batchRequest
	if err := dec.Decode(&req); err != nil {
		var sizeErr *http.MaxBytesError
		if errors.As(err, &sizeErr) {
			writeError(w, http.StatusRequestEntityTooLarge, fmt.Sprintf("тело больше %d байт", sizeErr.Limit))
			return
		}
		writeError(w, http.StatusBadRequest, "некорректный JSON: "+err.Error())
		return
	}
	if !namePattern.MatchString(req.Name) {
		writeError(w, http.StatusUnprocessableEntity, "name: 1–64 символа из A-Z, a-z, 0-9, _ и -")
		return
	}
	if n := len(req.Values); n == 0 || n > a.maxValues {
		writeError(w, http.StatusUnprocessableEntity, fmt.Sprintf("values: от 1 до %d чисел", a.maxValues))
		return
	}

	b := Batch{ID: uuid.NewString(), Name: req.Name, Values: req.Values, ReceivedAt: time.Now().UTC()}
	if err := a.spool.Enqueue(b); err != nil {
		a.log.Error("запись в volume", "error", err)
		writeError(w, http.StatusServiceUnavailable, "не удалось сохранить пакет")
		return
	}
	statusURL := "/api/v1/batches/" + b.ID
	w.Header().Set("Location", statusURL)
	writeJSON(w, http.StatusAccepted, map[string]any{
		"id": b.ID, "status": StatusQueued, "values": len(b.Values), "status_url": statusURL,
	})
}

func (a *API) batchStatus(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusBadRequest, "id должен быть UUID")
		return
	}
	st, err := a.spool.Status(id)
	switch {
	case errors.Is(err, ErrUnknownBatch):
		writeError(w, http.StatusNotFound, err.Error())
	case err != nil:
		writeError(w, http.StatusInternalServerError, "ошибка чтения volume")
	default:
		writeJSON(w, http.StatusOK, map[string]any{"id": id, "status": st})
	}
}

// statusRecorder запоминает код ответа для лога.
type statusRecorder struct {
	http.ResponseWriter
	status int
}

func (s *statusRecorder) WriteHeader(code int) {
	s.status = code
	s.ResponseWriter.WriteHeader(code)
}

func (a *API) logRequests(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		rec := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
		next.ServeHTTP(rec, r)
		if r.URL.Path == "/health/live" || r.URL.Path == "/health/ready" {
			return // healthcheck раз в несколько секунд засорил бы лог
		}
		a.log.Info("request", "method", r.Method, "path", r.URL.Path, "status", rec.status,
			"duration", time.Since(start).Round(time.Microsecond).String())
	})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}
