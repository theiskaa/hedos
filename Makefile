.PHONY: build test app run clean probe icon

build:
	swift build

test:
	swift test

app:
	./scripts/build_app.sh

run: app
	open dist/Hedos.app

probe:
	swift run hedos-probe

clean:
	rm -rf .build dist
icon:
	swift scripts/render_icon.swift
