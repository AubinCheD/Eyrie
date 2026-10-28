//go:build windows

package main

// L'exécutable que le veilleur lance (eyrie.exe, à côté de lui) doit être à l'abri des autres comptes du poste :
// s'ils pouvaient le remplacer, ou déposer à côté une DLL que Windows prendrait avant celles du système, un appui
// sur le téléphone ferait exécuter leur code sous le compte de l'utilisateur. C'est le cas d'un dossier créé à la
// racine d'un disque (`C:\Outils`), qui hérite d'un droit de modification pour tous les utilisateurs authentifiés ;
// `%LOCALAPPDATA%\Programs` et `Program Files` sont sûrs. Repris de ../Fawkes/src/exeplace.rs ; refait avant
// chaque lancement.

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"unsafe"

	"golang.org/x/sys/windows"
)

// Écrire, ajouter un fichier ou un sous-dossier, supprimer, changer les droits ou le propriétaire.
const writeRights = 0x2 | 0x4 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000

var trustedSIDs = []string{
	"S-1-5-18",     // SYSTEM
	"S-1-5-32-544", // Administrateurs
	"S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464", // TrustedInstaller
	"S-1-3-0", // CREATOR OWNER (le créateur d'un fichier, donc l'utilisateur ici)
	"S-1-3-4", // OWNER RIGHTS
}

// unsafePlace : l'exécutable ou son dossier `path` peut être modifié par le compte `who`.
type unsafePlace struct {
	path, who string
}

func (e *unsafePlace) Error() string {
	return fmt.Sprintf("%s peut être modifié par un autre compte du poste (%s) : le veilleur lancerait ce "+
		"qu'il y dépose. Installez Eyrie dans %%LOCALAPPDATA%%\\Programs ou dans Program Files.", e.path, e.who)
}

// exePlaceCheck rend une erreur, en français, si l'exécutable `exe` ou son dossier peut être modifié par un autre
// compte que l'utilisateur, SYSTEM, les administrateurs ou TrustedInstaller (*unsafePlace), si `exe` n'existe pas
// (fs.ErrNotExist) ou si leurs droits sont illisibles.
func exePlaceCheck(exe string) error {
	if _, err := os.Stat(exe); errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("%s introuvable : %w", exe, fs.ErrNotExist)
	}
	for _, path := range []string{exe, filepath.Dir(exe)} {
		who, err := writableByOthers(path)
		if err != nil {
			return err
		}
		if who != "" {
			return &unsafePlace{path: path, who: who}
		}
	}
	return nil
}

// writableByOthers : le premier compte (SID) autre que l'utilisateur et le système qui a un droit d'écriture sur
// `path`, ou "".
func writableByOthers(path string) (string, error) {
	me, err := userSID()
	if err != nil {
		return "", err
	}
	sd, err := windows.GetNamedSecurityInfo(path, windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION)
	if err != nil {
		return "", fmt.Errorf("droits de %s illisibles : %w", path, err)
	}
	dacl, _, err := sd.DACL()
	if errors.Is(err, windows.ERROR_OBJECT_NOT_FOUND) || (err == nil && dacl == nil) {
		// Pas de liste de contrôle : tout le monde a tous les droits.
		return "tout le monde", nil
	}
	if err != nil {
		return "", fmt.Errorf("droits de %s illisibles : %w", path, err)
	}
	for i := uint32(0); i < uint32(dacl.AceCount); i++ {
		var ace *windows.ACCESS_ALLOWED_ACE
		if windows.GetAce(dacl, i, &ace) != nil || ace == nil {
			continue
		}
		if ace.Header.AceType != windows.ACCESS_ALLOWED_ACE_TYPE ||
			ace.Header.AceFlags&windows.INHERIT_ONLY_ACE != 0 ||
			uint32(ace.Mask)&writeRights == 0 {
			continue
		}
		sid := (*windows.SID)(unsafe.Pointer(&ace.SidStart))
		if sid.Equals(me) || slices.Contains(trustedSIDs, sid.String()) {
			continue
		}
		return sid.String(), nil
	}
	return "", nil
}
