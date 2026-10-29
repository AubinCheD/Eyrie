//go:build windows

package main

// L'exécutable que le veilleur lance (eyrie.exe, à côté de lui) doit être à l'abri des autres comptes du poste :
// s'ils pouvaient le remplacer, ou déposer à côté une DLL que Windows prendrait avant celles du système, un appui
// sur le téléphone ferait exécuter leur code sous le compte de l'utilisateur. C'est le cas d'un dossier créé à la
// racine d'un disque (`C:\Outils`), qui hérite d'un droit de modification pour tous les utilisateurs authentifiés ;
// `%LOCALAPPDATA%\Programs` et `Program Files` sont sûrs. Le propriétaire de chacun compte aussi (il peut toujours
// changer les droits), et les dossiers au-dessus ne doivent pas pouvoir être renommés par un autre (il mettrait le
// sien à la place), par le chemin écrit, chaque jonction ou lien suivi comme Windows le suit, et le vrai ; une jonction
// ne doit pas non plus pouvoir être écrite (elle mènerait ailleurs). Repris de ../Fawkes/src/exeplace.rs ; refait avant
// chaque lancement. Fawkes pour ses démarrages, et Eyrie avant d'inscrire le veilleur
// (crates/eyrie/src/veille/exeplace.rs), appliquent les mêmes règles : les changer ensemble.

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"unsafe"

	"golang.org/x/sys/windows"
)

// Écrire, ajouter un fichier ou un sous-dossier, changer les attributs (FSCTL_SET_REPARSE_POINT : une jonction mène
// alors ailleurs), supprimer, changer les droits ou le propriétaire.
const writeRights = 0x2 | 0x4 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000

// Sur un dossier au-dessus : le renommer (supprimer), renommer ce qu'il contient (supprimer un enfant), changer les
// droits ou le propriétaire. Y créer un dossier ne suffit pas (c'est le droit de tous à la racine de C:).
const aboveRights = 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000

// Supprimer : la racine d'un disque ne se renomme pas.
const deleteRight = 0x1_0000

// Autant de liens que Windows en suit pour ouvrir un chemin : au-delà, refusé.
const maxLinks = 63

// Le bit des étiquettes de jonction et de lien (IsReparseTagNameSurrogate) : Windows suit ce point d'analyse vers un
// autre nom.
const nameSurrogate = 0x2000_0000

// ACCESS_ALLOWED_CALLBACK_ACE_TYPE (absent de golang.org/x/sys) : un droit accordé sous condition, compté comme
// accordé.
const allowedCallbackAce = 0x9

// Les comptes du système, sûrs comme l'utilisateur, propriétaires compris.
var systemSIDs = []string{
	"S-1-5-18",     // SYSTEM
	"S-1-5-32-544", // Administrateurs
	"S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464", // TrustedInstaller
}

// Dans une ACE, aussi le propriétaire : sûr, ou refusé avant (untrustedOwner).
var trustedSIDs = append(slices.Clone(systemSIDs),
	"S-1-3-0", // CREATOR OWNER (le créateur d'un fichier, donc l'utilisateur ici)
	"S-1-3-4", // OWNER RIGHTS
)

// unsafePlace : l'exécutable ou son dossier `path` peut être modifié par le compte `who`.
type unsafePlace struct {
	path, who string
}

func (e *unsafePlace) Error() string {
	return fmt.Sprintf("%s peut être modifié par un autre compte du poste (%s) : le veilleur lancerait ce "+
		"qu'il y dépose. Installez Eyrie dans %%LOCALAPPDATA%%\\Programs ou dans Program Files.", e.path, e.who)
}

// exePlaceCheck rend une erreur, en français, si l'exécutable `exe` ou son dossier peut être modifié par un autre
// compte que l'utilisateur, SYSTEM, les administrateurs ou TrustedInstaller, ou un dossier au-dessus renommé par lui,
// par le chemin écrit comme par le vrai (checkedLevels) (*unsafePlace), si `exe` n'existe pas (fs.ErrNotExist) ou si
// leurs droits sont illisibles.
func exePlaceCheck(exe string) error {
	if _, err := os.Stat(exe); errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("%s introuvable : %w", exe, fs.ErrNotExist)
	}
	levels, err := checkedLevels(exe)
	if err != nil {
		return err
	}
	for _, l := range levels {
		who, err := writableByOthers(l.path, l.rights)
		if err != nil {
			return err
		}
		if who != "" {
			return &unsafePlace{path: l.path, who: who}
		}
	}
	return nil
}

// placeLevel : un chemin contrôlé et les droits qu'aucun autre compte ne doit y avoir.
type placeLevel struct {
	path   string
	rights uint32
}

// placeLevels : l'exécutable `exe` et son dossier (writeRights), puis chaque dossier au-dessus jusqu'à la racine du
// disque (aboveRights, sans le droit de supprimer à la racine).
func placeLevels(exe string) []placeLevel {
	dir := filepath.Dir(exe)
	levels := []placeLevel{{exe, writeRights}, {dir, writeRights}}
	for path := dir; filepath.Dir(path) != path; {
		path = filepath.Dir(path)
		rights := uint32(aboveRights)
		if filepath.Dir(path) == path {
			rights &^= deleteRight
		}
		levels = append(levels, placeLevel{path, rights})
	}
	return levels
}

// checkedLevels : les niveaux de `exe` tel qu'écrit, puis, lien par lien, ceux du chemin où mène le premier lien ou
// jonction depuis la racine (linkTarget, suivi comme Windows le suit), puis ceux du vrai chemin (realPath), sans
// doublon. Windows lit les droits d'un lien, pas ceux du dossier où il mène : sans les chemins suivis, ce dossier, ceux
// au-dessus de lui et les liens de sa propre cible échapperaient. Un point d'analyse (lien, jonction) a les droits de
// writeRights : qui peut l'écrire le fait mener ailleurs sans le renommer.
func checkedLevels(exe string) ([]placeLevel, error) {
	var levels []placeLevel
	path := exe
	for range maxLinks + 1 {
		link, target := "", ""
		for _, l := range placeLevels(path) {
			name, err := windows.UTF16PtrFromString(l.path)
			if err != nil {
				return nil, err
			}
			attrs, err := windows.GetFileAttributes(name)
			if err != nil {
				return nil, fmt.Errorf("attributs de %s illisibles : %w", l.path, err)
			}
			if attrs&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
				l.rights = writeRights
				to, err := linkTarget(l.path)
				if err != nil {
					return nil, err
				}
				// Du fichier à la racine : le dernier trouvé est le premier que Windows suit.
				if to != "" {
					link, target = l.path, to
				}
			}
			if !slices.Contains(levels, l) {
				levels = append(levels, l)
			}
		}
		if link == "" {
			real, err := realPath(exe)
			if err != nil {
				return nil, fmt.Errorf("chemin réel de %s illisible : %w", exe, err)
			}
			for _, l := range placeLevels(real) {
				if !slices.Contains(levels, l) {
					levels = append(levels, l)
				}
			}
			return levels, nil
		}
		rest, err := filepath.Rel(link, path)
		if err != nil {
			return nil, err
		}
		path = filepath.Join(target, rest)
	}
	return nil, fmt.Errorf("%s : plus de %d liens à suivre", exe, maxLinks)
}

// linkTarget : où mène `link`, jonction ou lien symbolique, lu sans le suivre (FSCTL_GET_REPARSE_POINT) ; "" pour un
// autre point d'analyse (fichier compressé, OneDrive…), que Windows ne suit pas vers un autre nom. Une cible absolue
// est rendue sans `\\?\` quand c'est possible (winPath) ; relative, elle l'est au dossier du lien. Un lien d'un autre
// type, ou qui ne mène pas à un chemin `\??\…`, est refusé.
func linkTarget(link string) (string, error) {
	unreadable := func(err error) error { return fmt.Errorf("lien %s illisible : %w", link, err) }
	name, err := windows.UTF16PtrFromString(link)
	if err != nil {
		return "", err
	}
	h, err := windows.CreateFile(name, 0, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE,
		nil, windows.OPEN_EXISTING, windows.FILE_FLAG_OPEN_REPARSE_POINT|windows.FILE_FLAG_BACKUP_SEMANTICS, 0)
	if err != nil {
		return "", unreadable(err)
	}
	defer windows.CloseHandle(h)
	buf := make([]byte, windows.MAXIMUM_REPARSE_DATA_BUFFER_SIZE)
	var n uint32
	err = windows.DeviceIoControl(h, windows.FSCTL_GET_REPARSE_POINT, nil, 0, &buf[0], uint32(len(buf)), &n, nil)
	if err != nil {
		return "", unreadable(err)
	}
	// REPARSE_DATA_BUFFER : l'étiquette, des tailles, la place du nom à substituer (décalage et longueur en octets)
	// parmi les noms, qui suivent (après des drapeaux pour un lien symbolique).
	buf = buf[:n]
	if len(buf) < 8 {
		return "", unreadable(windows.ERROR_INVALID_DATA)
	}
	tag, names := binary.LittleEndian.Uint32(buf), 16
	switch {
	case tag == windows.IO_REPARSE_TAG_SYMLINK:
		names = 20
	case tag == windows.IO_REPARSE_TAG_MOUNT_POINT:
	case tag&nameSurrogate == 0:
		return "", nil
	default:
		return "", fmt.Errorf("lien %s d'un type inconnu (%#x)", link, tag)
	}
	if len(buf) < names {
		return "", unreadable(windows.ERROR_INVALID_DATA)
	}
	start := names + int(binary.LittleEndian.Uint16(buf[8:]))
	end := start + int(binary.LittleEndian.Uint16(buf[10:]))
	if end > len(buf) || (end-start)%2 != 0 {
		return "", unreadable(windows.ERROR_INVALID_DATA)
	}
	units := make([]uint16, (end-start)/2)
	for i := range units {
		units[i] = binary.LittleEndian.Uint16(buf[start+2*i:])
	}
	target := windows.UTF16ToString(units)
	// SYMLINK_FLAG_RELATIVE : relative au dossier du lien ; `\…` l'est à la racine de son disque.
	if tag == windows.IO_REPARSE_TAG_SYMLINK && binary.LittleEndian.Uint32(buf[16:])&1 != 0 {
		if strings.HasPrefix(target, `\`) {
			return filepath.Join(filepath.VolumeName(link)+`\`, target), nil
		}
		return filepath.Join(filepath.Dir(link), target), nil
	}
	if !strings.HasPrefix(target, `\??\`) {
		return "", fmt.Errorf("lien %s : cible %s illisible", link, target)
	}
	return winPath(`\\?\` + target[len(`\??\`):]), nil
}

// realPath : le vrai chemin de `path`, jonctions et liens résolus, par GetFinalPathNameByHandleW sur un handle ouvert
// avec FILE_FLAG_BACKUP_SEMANTICS (un dossier aussi) ; filepath.EvalSymlinks ne suit pas toujours les jonctions.
// Sans le préfixe `\\?\` quand c'est possible (winPath).
func realPath(path string) (string, error) {
	name, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return "", err
	}
	h, err := windows.CreateFile(name, 0, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE,
		nil, windows.OPEN_EXISTING, windows.FILE_FLAG_BACKUP_SEMANTICS, 0)
	if err != nil {
		return "", err
	}
	defer windows.CloseHandle(h)
	buf := make([]uint16, windows.MAX_LONG_PATH)
	// VOLUME_NAME_DOS | FILE_NAME_NORMALIZED (0) : `\\?\C:\…`.
	n, err := windows.GetFinalPathNameByHandle(h, &buf[0], uint32(len(buf)), 0)
	if err != nil {
		return "", err
	}
	if n >= uint32(len(buf)) {
		return "", windows.ERROR_NOT_ENOUGH_MEMORY
	}
	return winPath(windows.UTF16ToString(buf[:n])), nil
}

// winPath : `\\?\C:\…` en `C:\…`, `\\?\UNC\serveur\…` en `\\serveur\…`, si Windows lit chaque niveau pareil sans le
// préfixe ; sinon `real` tel quel. Sans lui, Windows retire un point ou une espace en fin de nom (`Outils.` devient
// `Outils`) et prend `NUL` pour un périphérique : un autre chemin serait contrôlé.
func winPath(real string) string {
	var short string
	switch {
	case strings.HasPrefix(real, `\\?\UNC\`):
		short = `\\` + real[len(`\\?\UNC\`):]
	case strings.HasPrefix(real, `\\?\`):
		short = real[len(`\\?\`):]
	default:
		return real
	}
	for path := short; ; path = filepath.Dir(path) {
		if full, err := windows.FullPath(path); err != nil || full != path {
			return real
		}
		if filepath.Dir(path) == path {
			return short
		}
	}
}

// writableByOthers : le propriétaire de `path`, s'il n'est ni l'utilisateur ni le système (« propriétaire <SID> »),
// sinon le premier compte (SID) autre que l'utilisateur et le système qui a l'un des droits `rights` sur `path`, ou "".
func writableByOthers(path string, rights uint32) (string, error) {
	me, err := userSID()
	if err != nil {
		return "", err
	}
	sd, err := windows.GetNamedSecurityInfo(path, windows.SE_FILE_OBJECT,
		windows.OWNER_SECURITY_INFORMATION|windows.DACL_SECURITY_INFORMATION)
	if err != nil {
		return "", fmt.Errorf("droits de %s illisibles : %w", path, err)
	}
	owner, _, err := sd.Owner()
	if err != nil {
		return "", fmt.Errorf("propriétaire de %s illisible : %w", path, err)
	}
	if who := untrustedOwner(owner, me); who != "" {
		return who, nil
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
		// Une ACE conditionnelle a la même tête, le même masque et le même SID, suivis de sa condition.
		if (ace.Header.AceType != windows.ACCESS_ALLOWED_ACE_TYPE && ace.Header.AceType != allowedCallbackAce) ||
			ace.Header.AceFlags&windows.INHERIT_ONLY_ACE != 0 ||
			uint32(ace.Mask)&rights == 0 {
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

// untrustedOwner : « propriétaire <SID> » si `owner` n'est ni l'utilisateur `me` ni un compte du système, sinon "".
// Le propriétaire d'un fichier ou d'un dossier peut toujours en changer les droits : un autre compte qui l'aurait
// créé (un dossier préparé pour l'utilisateur) s'y donnerait plus tard le droit d'écrire.
func untrustedOwner(owner, me *windows.SID) string {
	if owner == nil {
		return "propriétaire inconnu"
	}
	if owner.Equals(me) || slices.Contains(systemSIDs, owner.String()) {
		return ""
	}
	return "propriétaire " + owner.String()
}
