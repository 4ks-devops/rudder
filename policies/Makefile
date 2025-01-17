# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2020 Normation SAS

include ../rust.makefile

DOC_EXAMPLES := $(wildcard rudderc/docs/examples/*.yml)
DOC_EXAMPLES_CF := $(DOC_EXAMPLES:.yml=.cf)

install: build
	mkdir -p $(DESTDIR)/opt/rudder/bin
	# Install executable
	install -m 755 target/release/rudderc $(DESTDIR)/opt/rudder/bin/rudderc

test-docs: $(DOC_EXAMPLES_CF)

test: libs
	cargo test --locked
	# Parse lib
	cargo run --bin rudderc --quiet -- -o /tmp/test.cf rudderc/tests/cases/general/ntp.yml -l rudderc/repos/ncf/tree/30_generic_methods/ -l rudderc/repos/dsc/plugin/src/ncf/30_generic_methods/

%.cf: %.yml
	cargo run --bin rudderc --quiet -- --quiet --output $@ --target cf $<

docs: test-docs
	mkdir -p target/docs
	cp -r rudderc/docs/images target/docs/
	asciidoctor -a stylesheet=assets/css/rudder.css --doctype=book --destination-dir=target/docs rudderc/docs/index.adoc

libs:
	mkdir -p rudderc/repos
	[ -d rudderc/repos/ncf ] || git clone git@github.com:Normation/ncf.git rudderc/repos/ncf
	cd rudderc/repos/ncf && git checkout master && git pull origin master
	[ -d rudderc/repos/dsc ] || git clone git@github.com:Normation/rudder-agent-windows.git rudderc/repos/dsc || mkdir -p repos/dsc/plugin/ncf/30_generic_methods/
	cd rudderc/repos/dsc && git checkout master && git pull origin master || true

check: lint test test-docs

.PHONY: docs libs clean
