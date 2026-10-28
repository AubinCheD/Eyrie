//go:build windows

package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func icacls(t *testing.T, path string, args ...string) {
	t.Helper()
	if out, err := exec.Command("icacls", append([]string{path}, args...)...).CombinedOutput(); err != nil {
		t.Fatalf("icacls %v : %v\n%s", args, err, out)
	}
}

// Un dossier du profil est à l'abri ; ouvert en écriture aux utilisateurs authentifiés, il ne l'est plus, ni
// l'exécutable qu'il contient.
func TestAFolderOthersCanWriteToIsRefused(t *testing.T) {
	dir := t.TempDir()
	exe := filepath.Join(dir, "eyrie.exe")
	if err := os.WriteFile(exe, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if who, err := writableByOthers(dir); err != nil || who != "" {
		t.Fatalf("dossier d'essai : %q %v", who, err)
	}
	if err := exePlaceCheck(exe); err != nil {
		t.Fatal(err)
	}
	icacls(t, dir, "/grant", "*S-1-5-11:(OI)(CI)M")
	defer icacls(t, dir, "/remove:g", "*S-1-5-11")
	if who, err := writableByOthers(dir); err != nil || who != "S-1-5-11" {
		t.Fatalf("dossier ouvert : %q %v", who, err)
	}
	err := exePlaceCheck(exe)
	if err == nil || !strings.Contains(err.Error(), "S-1-5-11") || !strings.Contains(err.Error(), "Program Files") {
		t.Fatalf("refus : %v", err)
	}
}
