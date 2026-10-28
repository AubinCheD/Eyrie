//go:build windows

package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Les essais relancent leur propre exécutable dans un rôle (EYRIE_ESSAI_ROLE) : un autre processus que l'essai.
func TestMain(m *testing.M) {
	if role := os.Getenv("EYRIE_ESSAI_ROLE"); role != "" {
		os.Exit(helper(role))
	}
	os.Exit(m.Run())
}

func helper(role string) int {
	switch role {
	case "verrou":
		// Tient le nœud de EYRIE_ESSAI_DIR jusqu'à la fin de son entrée.
		lock, err := lockNode(os.Getenv("EYRIE_ESSAI_DIR"))
		if err != nil {
			return 1
		}
		defer lock.Close()
		os.Stdout.WriteString("tenu\n")
		_, _ = io.Copy(io.Discard, os.Stdin)
		return 0
	case "lance":
		return helperLaunched()
	case "veilleur":
		return helperStandby()
	}
	return 9
}

// helperProcess lance l'exécutable de l'essai dans le rôle `role`, avec `env` en plus.
func helperProcess(t *testing.T, role string, env ...string) *exec.Cmd {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(exe)
	cmd.Env = append(os.Environ(), append([]string{"EYRIE_ESSAI_ROLE=" + role}, env...)...)
	return cmd
}

// Le nom du verrou est celui que donne `dir_key` de suite-sdk : Eyrie le teste sous le même nom.
func TestTheLockIsNamedLikeSuiteSdk(t *testing.T) {
	key := dirKey(`C:\Users\Aubin\AppData\Local\EyrieSuite\Eyrie\tailnet\`)
	if key != "c:/users/aubin/appdata/local/eyriesuite/eyrie/tailnet" {
		t.Fatalf("dir_key : %q", key)
	}
	if got := lockName(`C:\X\Tailnet`); got != `Local\EyrieTailnet:c:/x/tailnet` {
		t.Fatalf("nom : %q", got)
	}
}

// Les minuscules sont celles de Rust (str::to_lowercase), pas celles de strings.ToLower : sigma final, İ. Vecteurs
// calculés par `dir_key` de suite-sdk (Rust 1.99).
func TestTheKeyLowercasesLikeRust(t *testing.T) {
	for dir, want := range map[string]string{
		`C:\Users\ΝΙΚΟΣ\AppData\Local\EyrieSuite\Eyrie\tailnet`:   "c:/users/νικος/appdata/local/eyriesuite/eyrie/tailnet",
		`C:\Users\İrem\AppData\Local\EyrieSuite\Eyrie\tailnet\`:   "c:/users/i\u0307rem/appdata/local/eyriesuite/eyrie/tailnet",
		`D:\ΟΣ.Ν\Σ\ΑΣ'Β\tailnet`:                                  "d:/οσ.ν/σ/ασ'β/tailnet",
		`C:\Users\Exemple\AppData\Local\EyrieSuite\Eyrie\tailnet`: "c:/users/exemple/appdata/local/eyriesuite/eyrie/tailnet",
	} {
		if got := dirKey(dir); got != want {
			t.Errorf("%s : %q, attendu %q", dir, got, want)
		}
	}
}

// Un second compagnon sur le même nœud (même dossier, à la casse et à la barre finale près) dit pourquoi il ne
// démarre pas et sort avec le code 3 ; le nœud libéré se reprend.
func TestASecondCompanionOnTheSameNodeIsRefused(t *testing.T) {
	dir := t.TempDir()
	holder := helperProcess(t, "verrou", "EYRIE_ESSAI_DIR="+strings.ToUpper(dir)+`\`)
	in, err := holder.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	out, err := holder.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := holder.Start(); err != nil {
		t.Fatal(err)
	}
	defer holder.Wait()
	defer in.Close()
	if line, err := bufio.NewReader(out).ReadString('\n'); err != nil || line != "tenu\n" {
		t.Fatalf("le premier compagnon ne tient pas le nœud : %q %v", line, err)
	}

	var buf bytes.Buffer
	saved := status
	status = &statusLines{w: &buf}
	defer func() { status = saved }()
	if code := run([]string{"--dir", dir}); code != exitHeld {
		t.Fatalf("code de sortie : %d", code)
	}
	var line map[string]string
	if err := json.Unmarshal(buf.Bytes(), &line); err != nil {
		t.Fatalf("ligne d'état : %q %v", buf.String(), err)
	}
	if line["state"] != "error" || line["message"] != "nœud déjà tenu par un autre compagnon" {
		t.Fatalf("ligne d'état : %v", line)
	}
	if _, err := os.Stat(dir + `\journal.txt`); err == nil {
		t.Fatal("le second compagnon a touché au journal du premier")
	}

	in.Close()
	if err := holder.Wait(); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		lock, err := lockNode(dir)
		if err == nil {
			lock.Close()
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("le nœud libéré ne se reprend pas : %v", err)
		}
		time.Sleep(50 * time.Millisecond)
	}
}

// statusTo envoie les lignes d'état dans `buf` le temps de l'essai.
func statusTo(t *testing.T, buf *bytes.Buffer) {
	saved := status
	status = &statusLines{w: buf}
	t.Cleanup(func() { status = saved })
}

// Le veilleur sans dossier du pont mobile ne démarre pas (code 2), et dit pourquoi.
func TestAStandbyNeedsTheMobileFolder(t *testing.T) {
	var buf bytes.Buffer
	statusTo(t, &buf)
	if code := run([]string{"--veille", "--dir", t.TempDir()}); code != exitUsage {
		t.Fatalf("code de sortie : %d", code)
	}
	if !strings.Contains(buf.String(), "--mobile") {
		t.Fatalf("ligne d'état : %q", buf.String())
	}
}

// Un veilleur qui échoue après avoir pris le nœud (ici, un état du nœud indéchiffrable) le laisse dans son dossier
// (veille-echec.json) et dans son journal : lancé à l'ouverture de session, personne ne l'écoutait.
func TestAStandbyThatFailsLeavesWhy(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "noeud.dpapi"), []byte("pas du DPAPI"), 0o600); err != nil {
		t.Fatal(err)
	}
	var buf bytes.Buffer
	statusTo(t, &buf)
	if code := run([]string{"--veille", "--dir", dir, "--mobile", t.TempDir()}); code != exitFailed {
		t.Fatalf("code de sortie : %d", code)
	}
	raw, err := os.ReadFile(filepath.Join(dir, standbyFailFile))
	if err != nil {
		t.Fatal(err)
	}
	var why standbyFailure
	if err := json.Unmarshal(raw, &why); err != nil || why.At == 0 || !strings.Contains(why.Message, "indéchiffrable") {
		t.Fatalf("%s : %q %v", standbyFailFile, raw, err)
	}
	if logs, _ := os.ReadFile(filepath.Join(dir, "journal.txt")); !strings.Contains(string(logs), "échec : état du nœud") {
		t.Fatalf("journal : %q", logs)
	}
	if !strings.Contains(buf.String(), "indéchiffrable") {
		t.Fatalf("ligne d'état : %q", buf.String())
	}
}

// Dans un même processus aussi, le verrou ne se prend qu'une fois.
func TestTheLockIsHeldOnce(t *testing.T) {
	dir := t.TempDir()
	first, err := lockNode(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := lockNode(dir); err != errNodeHeld {
		t.Fatalf("pris deux fois : %v", err)
	}
	first.Close()
	again, err := lockNode(dir)
	if err != nil {
		t.Fatal(err)
	}
	again.Close()
}
