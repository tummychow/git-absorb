build-documentation: git-absorb.1 git-absorb.html

git-absorb.1: git-absorb.adoc
	$(if $(shell command -v a2x),,$(error "No a2x in PATH; install asciidoc."))
	$(info Building manpage. This may take a few moments...)
	a2x -L -d manpage -f manpage git-absorb.adoc --attribute man-version=${GA_VERSION}

git-absorb.html: git-absorb.adoc
	$(if $(shell command -v a2x),,$(error "No a2x in PATH; install asciidoc."))

	$(info Building HTML docs. This may take a few moments...)
	asciidoc -d manpage -b xhtml11 git-absorb.adoc

clean:
	rm -f git-absorb.1 git-absorb.xml
