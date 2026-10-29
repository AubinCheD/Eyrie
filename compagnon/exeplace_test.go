//go:build windows

package main

import (
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"golang.org/x/sys/windows"
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
	if who, err := writableByOthers(dir, writeRights); err != nil || who != "" {
		t.Fatalf("dossier d'essai : %q %v", who, err)
	}
	if err := exePlaceCheck(exe); err != nil {
		t.Fatal(err)
	}
	icacls(t, dir, "/grant", "*S-1-5-11:(OI)(CI)M")
	defer icacls(t, dir, "/remove:g", "*S-1-5-11")
	if who, err := writableByOthers(dir, writeRights); err != nil || who != "S-1-5-11" {
		t.Fatalf("dossier ouvert : %q %v", who, err)
	}
	err := exePlaceCheck(exe)
	var place *unsafePlace
	if !errors.As(err, &place) || !strings.Contains(err.Error(), "S-1-5-11") ||
		!strings.Contains(err.Error(), "Program Files") {
		t.Fatalf("refus : %v", err)
	}
}

// Un dossier au-dessus qu'un autre compte peut renommer, ou dont il peut renommer le contenu, ne protège rien : il
// mettrait son propre dossier à la place. Y créer un dossier (comme à la racine de C:) ne suffit pas pour cela.
func TestAFolderAboveOthersCanRenameIsRefused(t *testing.T) {
	above := filepath.Join(t.TempDir(), "Outils")
	exe := filepath.Join(above, "Eyrie", "eyrie.exe")
	if err := os.MkdirAll(filepath.Dir(exe), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(exe, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	// Des droits sur ce seul dossier : ni Eyrie ni eyrie.exe n'en héritent.
	for _, right := range []string{"DE", "DC", "WDAC", "WO"} {
		icacls(t, above, "/grant", "*S-1-5-11:("+right+")")
		err := exePlaceCheck(exe)
		var place *unsafePlace
		if !errors.As(err, &place) || place.path != above || place.who != "S-1-5-11" {
			t.Fatalf("%s : %v", right, err)
		}
		icacls(t, above, "/remove:g", "*S-1-5-11")
	}
	icacls(t, above, "/grant", "*S-1-5-11:(AD)")
	defer icacls(t, above, "/remove:g", "*S-1-5-11")
	if err := exePlaceCheck(exe); err != nil {
		t.Fatalf("créer un dossier au-dessus : %v", err)
	}
}

// Le propriétaire peut toujours se donner tous les droits : seuls l'utilisateur et le système sont des propriétaires
// sûrs (un compte ordinaire ne peut pas donner un fichier à un autre ; l'essai ne peut donc que les comparer).
func TestOnlyTheUserOrTheSystemOwnsASafePlace(t *testing.T) {
	me, err := userSID()
	if err != nil {
		t.Fatal(err)
	}
	sid := func(s string) *windows.SID {
		x, err := windows.StringToSid(s)
		if err != nil {
			t.Fatal(err)
		}
		return x
	}
	for _, owner := range []*windows.SID{me, sid("S-1-5-18"), sid("S-1-5-32-544"),
		sid("S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464")} {
		if who := untrustedOwner(owner, me); who != "" {
			t.Errorf("%s : %s", owner, who)
		}
	}
	other := "S-1-5-21-1004336348-1177238915-682003330-1001"
	if who := untrustedOwner(sid(other), me); who != "propriétaire "+other {
		t.Errorf("autre compte : %q", who)
	}
	// OWNER RIGHTS, sûr dans une ACE, ne l'est pas comme propriétaire ; sans propriétaire, rien n'est sûr.
	for _, owner := range []*windows.SID{sid("S-1-3-4"), sid("S-1-5-11"), nil} {
		if who := untrustedOwner(owner, me); who == "" {
			t.Errorf("%v accepté", owner)
		}
	}
	// Le dossier d'essai, créé par l'utilisateur, est le sien.
	sd, err := windows.GetNamedSecurityInfo(t.TempDir(), windows.SE_FILE_OBJECT, windows.OWNER_SECURITY_INFORMATION)
	if err != nil {
		t.Fatal(err)
	}
	if owner, _, err := sd.Owner(); err != nil || untrustedOwner(owner, me) != "" {
		t.Fatalf("propriétaire du dossier d'essai : %v %v", owner, err)
	}
}

// Un droit accordé sous condition (ACE conditionnelle) compte comme accordé.
func TestAConditionalGrantCounts(t *testing.T) {
	dir := t.TempDir()
	me, err := userSID()
	if err != nil {
		t.Fatal(err)
	}
	sd, err := windows.SecurityDescriptorFromString("D:P(A;OICI;FA;;;" + me.String() + ")" +
		"(XA;;FA;;;AU;(Member_of {SID(BA)}))")
	if err != nil {
		t.Fatal(err)
	}
	dacl, _, err := sd.DACL()
	if err != nil {
		t.Fatal(err)
	}
	if err := windows.SetNamedSecurityInfo(dir, windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION, nil, nil, dacl, nil); err != nil {
		t.Fatal(err)
	}
	if who, err := writableByOthers(dir, writeRights); err != nil || who != "S-1-5-11" {
		t.Fatalf("droit sous condition : %q %v", who, err)
	}
}

// Un eyrie.exe absent est refusé, mais n'est pas dit modifiable par un autre compte.
func TestAMissingEyrieIsNotCalledUnsafe(t *testing.T) {
	err := exePlaceCheck(filepath.Join(t.TempDir(), "eyrie.exe"))
	var place *unsafePlace
	if !errors.Is(err, fs.ErrNotExist) || errors.As(err, &place) || !strings.Contains(err.Error(), "introuvable") {
		t.Fatalf("refus : %v", err)
	}
	if got := placeReason(err); !strings.Contains(got, "introuvable") {
		t.Fatalf("raison : %s", got)
	}
}
