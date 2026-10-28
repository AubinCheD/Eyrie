package noeud

import (
	"io"
	"net"
	"testing"
	"time"
)

func TestRelierCopiesBothWaysAndClosesTheOtherEnd(t *testing.T) {
	phone, a := net.Pipe()
	b, pc := net.Pipe()
	done := make(chan struct{})
	go func() {
		Relier(a, b)
		close(done)
	}()

	go phone.Write([]byte("bonjour"))
	buf := make([]byte, 7)
	if _, err := io.ReadFull(pc, buf); err != nil || string(buf) != "bonjour" {
		t.Fatalf("téléphone → PC : %q, %v", buf, err)
	}
	go pc.Write([]byte("salut"))
	buf = make([]byte, 5)
	if _, err := io.ReadFull(phone, buf); err != nil || string(buf) != "salut" {
		t.Fatalf("PC → téléphone : %q, %v", buf, err)
	}

	// Le PC raccroche : le téléphone le voit, et le relais s'arrête.
	pc.Close()
	if _, err := phone.Read(buf); err == nil {
		t.Fatal("le bout du téléphone devrait être fermé")
	}
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("le relais ne s'est pas arrêté")
	}
}
