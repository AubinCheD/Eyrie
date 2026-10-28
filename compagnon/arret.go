//go:build windows

package main

// --arreter : arrêter les veilleurs qui tournent depuis cet exécutable, par leur tube, comme Eyrie le ferait. Un
// veilleur en marche verrouille son exécutable : tools\compagnon.ps1 l'arrête avant de le remplacer.

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/tailscale/go-winio"
	"golang.org/x/sys/windows"
)

// Le temps laissé à un veilleur pour sortir (fermer son nœud) une fois l'arrêt accepté.
const stopWait = 15 * time.Second

// stopStandby arrête le veilleur du nœud `dir`, ou, sans dossier, tous les veilleurs de l'utilisateur qui tournent
// depuis cet exécutable, et attend qu'ils soient sortis. Rend 0 si tous sont arrêtés (ou s'il n'y en a pas), 1 sinon ;
// les tubes non reconnus sont signalés sur la sortie d'erreur, sans échec.
func stopStandby(dir string) int {
	self, err := os.Executable()
	if err != nil {
		fmt.Fprintf(os.Stderr, "exécutable du compagnon : %v\n", err)
		return exitFailed
	}
	names := standbyPipes()
	if dir != "" {
		names = []string{pipeName(dir)}
	}
	code := exitOK
	for _, name := range names {
		stopped, err := stopOne(name, self, stopWait)
		switch {
		case errors.Is(err, errForeign):
			fmt.Fprintf(os.Stderr, "tube %s ignoré : %v\n", name, err)
		case err != nil:
			fmt.Fprintf(os.Stderr, "veilleur %s : %v\n", name, err)
			code = exitFailed
		case stopped:
			fmt.Printf("veilleur arrêté (%s)\n", name)
		}
	}
	return code
}

// standbyPipes : les tubes de veilleurs ouverts sur ce poste.
func standbyPipes() []string {
	var data windows.Win32finddata
	pattern, _ := windows.UTF16PtrFromString(`\\.\pipe\*`)
	h, err := windows.FindFirstFile(pattern, &data)
	if err != nil {
		return nil
	}
	defer windows.FindClose(h)
	var out []string
	for {
		if name := windows.UTF16ToString(data.FileName[:]); strings.HasPrefix(name, "eyrie-tailnet-") {
			out = append(out, `\\.\pipe\`+name)
		}
		if windows.FindNextFile(h, &data) != nil {
			return out
		}
	}
}

// errForeign : rien ne dit que ce tube est celui d'un veilleur lancé depuis cet exécutable. Les tubes sont communs à
// tout le poste : ceux des veilleurs d'autres comptes (changement rapide d'utilisateur) y sont, fermés à celui-ci.
var errForeign = errors.New("veilleur non reconnu")

// stopOne demande l'arrêt au veilleur du tube `name` s'il tourne depuis l'exécutable `self`, puis attend qu'il soit
// sorti (`wait` au plus). Rend faux, sans erreur, si personne n'écoute ce tube ou si son veilleur vient d'ailleurs ;
// errForeign si son serveur ne peut pas être reconnu (tube d'un autre compte, occupé, processus illisible). Les
// autres erreurs viennent d'un veilleur reconnu qui ne s'arrête pas.
func stopOne(name, self string, wait time.Duration) (bool, error) {
	timeout := 2 * time.Second
	c, err := winio.DialPipe(name, &timeout)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, fmt.Errorf("%w : %v", errForeign, err)
	}
	defer c.Close()
	h, err := pipeHandle(c)
	if err != nil {
		return false, fmt.Errorf("%w : %v", errForeign, err)
	}
	var pid uint32
	if err := windows.GetNamedPipeServerProcessId(h, &pid); err != nil {
		return false, fmt.Errorf("%w : processus du tube illisible : %v", errForeign, err)
	}
	proc, err := windows.OpenProcess(windows.SYNCHRONIZE|windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if err != nil {
		return false, fmt.Errorf("%w : processus %d : %v", errForeign, pid, err)
	}
	defer windows.CloseHandle(proc)
	image, err := processImage(proc)
	if err != nil {
		return false, fmt.Errorf("%w : exécutable du processus %d illisible : %v", errForeign, pid, err)
	}
	if !sameFile(image, self) {
		return false, nil
	}
	c.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := c.Write([]byte(`{"arret":true}` + "\n")); err != nil {
		return false, err
	}
	accepted := false
	for sc := lines(c); sc.Scan(); {
		var m map[string]string
		if json.Unmarshal(sc.Bytes(), &m) == nil && m["state"] == "stopped" {
			accepted = true
			break
		}
	}
	if !accepted {
		return false, errors.New("le veilleur n'a pas accepté l'arrêt")
	}
	if ev, err := windows.WaitForSingleObject(proc, uint32(wait/time.Millisecond)); err != nil || ev != windows.WAIT_OBJECT_0 {
		return false, errors.New("le veilleur ne s'est pas arrêté")
	}
	return true, nil
}
