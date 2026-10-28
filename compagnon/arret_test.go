//go:build windows

package main

import (
	"bufio"
	"context"
	"io"
	"net"
	"os"
	"path/filepath"
	"slices"
	"sync"
	"testing"
	"time"
)

// Rôle « veilleur » : le tube d'un veilleur sur EYRIE_ESSAI_DIR, qui accepte son propre exécutable, jusqu'à l'arrêt
// demandé (code 0) ou la fin de son entrée (code 7).
func helperStandby() int {
	exe, err := os.Executable()
	if err != nil {
		return 1
	}
	ln, err := listenControl(os.Getenv("EYRIE_ESSAI_DIR"))
	if err != nil {
		return 1
	}
	ctx, cancel := context.WithCancel(context.Background())
	tb := &tube{
		relay: &relay{},
		check: func(c net.Conn) error { return checkClient(c, []string{exe}) },
		stop:  cancel,
		logf:  func(string, ...any) {},
	}
	go tb.serve(ctx, ln)
	ended := make(chan struct{})
	go func() {
		_, _ = io.Copy(io.Discard, os.Stdin)
		close(ended)
	}()
	os.Stdout.WriteString("pret\n")
	select {
	case <-ctx.Done():
		return 0
	case <-ended:
		return 7
	}
}

// startStandby lance un veilleur d'essai (l'exécutable `exe`) sur `dir`.
func startStandby(t *testing.T, exe, dir string) (wait func() int, closeIn func()) {
	t.Helper()
	cmd := helperProcess(t, "veilleur", "EYRIE_ESSAI_DIR="+dir)
	cmd.Path = exe
	cmd.Args[0] = exe
	in, err := cmd.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	out, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	exited := sync.OnceValue(func() int {
		_ = cmd.Wait()
		return cmd.ProcessState.ExitCode()
	})
	t.Cleanup(func() {
		in.Close()
		exited()
	})
	if line, err := bufio.NewReader(out).ReadString('\n'); err != nil || line != "pret\n" {
		t.Fatalf("veilleur d'essai : %q %v", line, err)
	}
	return exited, func() { in.Close() }
}

// --arreter arrête le veilleur lancé depuis le même exécutable, et attend qu'il soit sorti ; sans veilleur, rien à
// faire.
func TestStopEndsAStandbyFromTheSameExecutable(t *testing.T) {
	dir := t.TempDir()
	wait, _ := startStandby(t, self(t), dir)
	if !slices.Contains(standbyPipes(), pipeName(dir)) {
		t.Fatalf("tube du veilleur absent : %v", standbyPipes())
	}
	if code := stopStandby(dir); code != exitOK {
		t.Fatalf("code : %d", code)
	}
	done := make(chan int, 1)
	go func() { done <- wait() }()
	select {
	case code := <-done:
		if code != 0 {
			t.Fatalf("le veilleur est sorti avec le code %d", code)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("le veilleur tourne encore")
	}
	if code := stopStandby(dir); code != exitOK {
		t.Fatalf("sans veilleur : code %d", code)
	}
}

// Un veilleur lancé depuis un autre exécutable (un autre dossier de compilation) n'est pas arrêté.
func TestStopLeavesAStandbyFromElsewhere(t *testing.T) {
	src, err := os.ReadFile(self(t))
	if err != nil {
		t.Fatal(err)
	}
	other := filepath.Join(t.TempDir(), "autre-veilleur.exe")
	if err := os.WriteFile(other, src, 0o700); err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	wait, closeIn := startStandby(t, other, dir)
	if stopped, err := stopOne(pipeName(dir), self(t), time.Second); stopped || err != nil {
		t.Fatalf("arrêté : %v %v", stopped, err)
	}
	closeIn()
	if code := wait(); code != 7 {
		t.Fatalf("le veilleur d'ailleurs est sorti avec le code %d", code)
	}
}
