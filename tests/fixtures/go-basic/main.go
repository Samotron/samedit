// Package main greets the world for the go-basic cockpit fixture.
package main

import "fmt"

// Greeter renders a name as a greeting.
type Greeter interface {
	Hello() string
}

// World is the canonical greeter target.
type World struct {
	Name string
}

// Hello returns "hello <name>".
func (w *World) Hello() string {
	return fmt.Sprintf("hello %s", w.Name)
}

func main() {
	w := &World{Name: "world"}
	fmt.Println(w.Hello())
}
