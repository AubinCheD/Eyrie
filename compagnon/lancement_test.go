//go:build windows

package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

type launched struct {
	Args      []string `json:"args"`
	Dir       string   `json:"dir"`
	By        string   `json:"par"`
	Inherited string   `json:"herite"`
}

// Rôle « lance » : dit ce qu'il a reçu dans EYRIE_ESSAI_SORTIE.
func helperLaunched() int {
	wd, _ := os.Getwd()
	b, _ := json.Marshal(launched{
		Args:      os.Args[1:],
		Dir:       wd,
		By:        os.Getenv("EYRIE_DEMARRE_PAR"),
		Inherited: os.Getenv("EYRIE_ESSAI_HERITE"),
	})
	if err := os.WriteFile(os.Getenv("EYRIE_ESSAI_SORTIE"), b, 0o600); err != nil {
		return 1
	}
	return 0
}

// Eyrie lancée reçoit ses arguments et le nom de l'appareil, travaille dans son dossier, et n'hérite pas de
// l'environnement du veilleur.
func TestEyrieIsLaunchedWithAFreshEnvironment(t *testing.T) {
	exe := self(t)
	out := filepath.Join(t.TempDir(), "lance.json")
	t.Setenv("EYRIE_ESSAI_HERITE", "oui")
	err := launchEyrie(exe, []string{"--reduite"}, []string{
		"EYRIE_ESSAI_ROLE=lance",
		"EYRIE_ESSAI_SORTIE=" + out,
		"EYRIE_DEMARRE_PAR=Pixel 8",
	})
	if err != nil {
		t.Fatal(err)
	}
	var got launched
	eventually(t, "Eyrie d'essai pas lancée", func() bool {
		b, err := os.ReadFile(out)
		return err == nil && json.Unmarshal(b, &got) == nil
	})
	if !slices.Equal(got.Args, []string{"--reduite"}) || got.By != "Pixel 8" {
		t.Fatalf("reçu : %+v", got)
	}
	if got.Inherited != "" {
		t.Fatal("l'environnement du veilleur a été hérité")
	}
	if !sameFile(got.Dir, filepath.Dir(exe)) {
		t.Fatalf("dossier de travail : %s", got.Dir)
	}
}

func TestExtraVariablesReplaceTheirNamesakes(t *testing.T) {
	got := withEnv([]string{`=C:=C:\`, `Path=C:\Windows`, "eyrie_demarre_par=ancien"}, []string{"EYRIE_DEMARRE_PAR=Pixel"})
	want := []string{`=C:=C:\`, `Path=C:\Windows`, "EYRIE_DEMARRE_PAR=Pixel"}
	if !slices.Equal(got, want) {
		t.Fatalf("%q", got)
	}
	block, err := envBlock([]string{"A=1", "B=2"})
	if err != nil || string(utf16ToBytes(block)) != "A=1\x00B=2\x00\x00" {
		t.Fatalf("bloc : %v %v", block, err)
	}
	if _, err := envBlock([]string{"A=\x00"}); err == nil {
		t.Fatal("un zéro dans une variable est accepté")
	}
}

func utf16ToBytes(w []uint16) []byte {
	b := make([]byte, len(w))
	for i, c := range w {
		b[i] = byte(c)
	}
	return b
}
