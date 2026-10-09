// ingest — Go-сервис приёма пакетов измерений. Складывает их в общий volume,
// откуда их забирает processor (Rust).
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"syscall"
	"time"
)

func main() {
	healthcheck := flag.Bool("healthcheck", false, "проверить /health/ready и выйти (Docker HEALTHCHECK в образе без curl)")
	flag.Parse()

	addr := env("ADDR", ":8080")
	if *healthcheck {
		os.Exit(probe(addr))
	}

	log := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	if err := run(addr, log); err != nil {
		log.Error("сервис завершился с ошибкой", "error", err)
		os.Exit(1)
	}
}

func run(addr string, log *slog.Logger) error {
	maxValues, err := strconv.Atoi(env("MAX_VALUES", "100000"))
	if err != nil || maxValues < 1 {
		return errors.New("MAX_VALUES: ожидается положительное целое")
	}
	spool, err := NewSpool(env("DATA_DIR", "/data"))
	if err != nil {
		return err
	}
	api := &API{
		spool:     spool,
		maxValues: maxValues,
		maxBody:   int64(maxValues)*32 + 4096, // ~32 байта на число в JSON с запасом
		log:       log,
	}
	srv := &http.Server{
		Addr:              addr,
		Handler:           api.Routes(),
		ReadHeaderTimeout: 5 * time.Second,
		ReadTimeout:       30 * time.Second,
		WriteTimeout:      30 * time.Second,
		IdleTimeout:       60 * time.Second,
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	errCh := make(chan error, 1)
	go func() {
		log.Info("ingest запущен", "addr", addr, "data_dir", spool.root)
		errCh <- srv.ListenAndServe()
	}()

	select {
	case err := <-errCh:
		return err
	case <-ctx.Done():
	}
	log.Info("получен сигнал, останавливаемся")
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if err := srv.Shutdown(shutdownCtx); err != nil {
		return fmt.Errorf("shutdown: %w", err)
	}
	log.Info("ingest остановлен")
	return nil
}

func env(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func probe(addr string) int {
	_, port, err := net.SplitHostPort(addr)
	if err != nil {
		return 1
	}
	client := http.Client{Timeout: 2 * time.Second}
	resp, err := client.Get("http://127.0.0.1:" + port + "/health/ready")
	if err != nil {
		return 1
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return 1
	}
	return 0
}
