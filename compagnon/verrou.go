//go:build windows

package main

// Un seul compagnon par nœud. Deux processus sur le même état (donc la même clé) se disputeraient le nœud sur le
// tailnet (le serveur de contrôle les ferait alterner, les connexions tomberaient) et l'écriture de son état
// (`etat.go` : même fichier temporaire, puis renommage). Le verrou est un mutex nommé de la session, au nom du
// dossier du nœud : Eyrie le teste avant de lancer un compagnon ou de prendre celui de Fawkes en secours, et se
// rattache au veilleur qui le tient (`tube.go`).

import (
	"errors"
	"strings"

	"golang.org/x/sys/windows"
)

// errNodeHeld : un autre compagnon tient déjà le nœud.
var errNodeHeld = errors.New("nœud déjà tenu par un autre compagnon")

// dirKey nomme un dossier pour un objet du noyau, comme `dir_key` de crates/suite-sdk/src/instance.rs : barres
// obliques, sans barre finale, minuscules.
func dirKey(dir string) string {
	return strings.ToLower(strings.TrimRight(strings.ReplaceAll(dir, `\`, "/"), "/"))
}

// lockName : le nom du verrou du nœud dont l'état est dans `dir`.
func lockName(dir string) string {
	return `Local\EyrieTailnet:` + dirKey(dir)
}

// nodeLock : le verrou tenu, jusqu'à Close (ou la fin du processus).
type nodeLock struct {
	h windows.Handle
}

// lockNode prend le verrou du nœud `dir` ; errNodeHeld s'il est déjà pris, y compris par ce processus.
func lockNode(dir string) (*nodeLock, error) {
	name, err := windows.UTF16PtrFromString(lockName(dir))
	if err != nil {
		return nil, err
	}
	h, err := windows.CreateMutex(nil, false, name)
	if errors.Is(err, windows.ERROR_ALREADY_EXISTS) {
		windows.CloseHandle(h)
		return nil, errNodeHeld
	}
	if err != nil {
		return nil, err
	}
	return &nodeLock{h: h}, nil
}

func (l *nodeLock) Close() {
	if l.h != 0 {
		windows.CloseHandle(l.h)
		l.h = 0
	}
}
