//go:build windows

package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"golang.org/x/sys/windows"
)

// Lancé en administrateur, le veilleur ne tourne pas : il démarrerait Eyrie en administrateur à la demande d'un
// téléphone. Il le dit dans veille-echec.json, au journal du pont et à qui l'écoute, et sort en échec, avant d'ouvrir
// son tube ou le nœud.
func TestAnElevatedStandbyRefusesToRun(t *testing.T) {
	was := elevated
	elevated = func() bool { return true }
	t.Cleanup(func() { elevated = was })
	var out bytes.Buffer
	lines := status
	status = &statusLines{w: &out}
	t.Cleanup(func() { status = lines })
	node, mobile := t.TempDir(), t.TempDir()

	if code := run([]string{"--veille", "--dir", node, "--mobile", mobile, "--hostname", "essai"}); code != exitFailed {
		t.Fatalf("code de sortie : %d", code)
	}
	raw, err := os.ReadFile(filepath.Join(node, standbyFailFile))
	var f standbyFailure
	if err != nil || json.Unmarshal(raw, &f) != nil || f.At == 0 || !strings.Contains(f.Message, "administrateur") {
		t.Fatalf("%s : %q %v", standbyFailFile, raw, err)
	}
	j := readJournal(t, mobile)
	if len(j) != 1 || j[0].Kind != "veille" || j[0].Device != "" || !strings.HasSuffix(j[0].Text, f.Message) {
		t.Fatalf("journal : %+v", j)
	}
	if !strings.Contains(out.String(), `"state":"error"`) || !strings.Contains(out.String(), "administrateur") {
		t.Fatalf("lignes d'état : %s", out.String())
	}
	entries, _ := os.ReadDir(node)
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	if !slices.Equal(names, []string{"journal.txt", standbyFailFile}) {
		t.Fatalf("dossier du nœud : %v", names)
	}
}

// Le jeton du veilleur se lit comme le lit golang.org/x/sys/windows.
func TestTheStandbyReadsItsOwnElevation(t *testing.T) {
	if got, want := tokenElevated(), windows.GetCurrentProcessToken().IsElevated(); got != want {
		t.Fatalf("élevé : %v, x/sys : %v", got, want)
	}
}
