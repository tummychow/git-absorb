build-manpage: git-absorb.1

git-absorb.1: git-absorb.txt
	$(if $(shell which a2x),,$(error "No a2x in PATH; install asciidoc."))
	$(info Building manpage. This may take a few moments...)
	a2x -L -d manpage -f manpage git-absorb.txt

clean:
	rm -f git-absorb.1 git-absorb.xml
