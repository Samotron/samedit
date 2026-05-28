package main

import "testing"

func TestWorldHello(t *testing.T) {
	w := &World{Name: "cockpit"}
	got := w.Hello()
	want := "hello cockpit"
	if got != want {
		t.Fatalf("Hello() = %q, want %q", got, want)
	}
}
