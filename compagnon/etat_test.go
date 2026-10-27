//go:build windows

package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"

	"tailscale.com/ipn"
)

// L'état écrit se relit après réouverture, et rien n'est en clair sur le disque.
func TestStateIsSealedOnDisk(t *testing.T) {
	dir := t.TempDir()
	s, err := openStateStore(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.ReadState("absent"); !errors.Is(err, ipn.ErrStateNotExist) {
		t.Fatalf("clé absente : %v", err)
	}
	secret := []byte("cle-privee-du-noeud-0123456789")
	if err := s.WriteState("_machinekey", secret); err != nil {
		t.Fatal(err)
	}
	onDisk, err := os.ReadFile(filepath.Join(dir, sealedFile))
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(onDisk, secret) || bytes.Contains(onDisk, []byte("_machinekey")) {
		t.Fatal("l'état est lisible en clair sur le disque")
	}
	again, err := openStateStore(dir)
	if err != nil {
		t.Fatal(err)
	}
	if got, err := again.ReadState("_machinekey"); err != nil || !bytes.Equal(got, secret) {
		t.Fatalf("relu : %q, %v", got, err)
	}
	if err := again.WriteState("_machinekey", nil); err != nil {
		t.Fatal(err)
	}
	if _, err := again.ReadState("_machinekey"); !errors.Is(err, ipn.ErrStateNotExist) {
		t.Fatalf("clé effacée : %v", err)
	}
}

// L'ancien état en clair de tsnet est repris, chiffré, puis effacé.
func TestPlainStateIsTakenOverThenRemoved(t *testing.T) {
	dir := t.TempDir()
	old, _ := json.Marshal(map[ipn.StateKey][]byte{"_machinekey": []byte("ancienne-cle")})
	if err := os.WriteFile(filepath.Join(dir, plainFile), old, 0o600); err != nil {
		t.Fatal(err)
	}
	s, err := openStateStore(dir)
	if err != nil {
		t.Fatal(err)
	}
	if got, err := s.ReadState("_machinekey"); err != nil || string(got) != "ancienne-cle" {
		t.Fatalf("repris : %q, %v", got, err)
	}
	if _, err := os.Stat(filepath.Join(dir, plainFile)); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("l'ancien fichier en clair reste : %v", err)
	}
}
