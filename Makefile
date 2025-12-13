SOURCES := $(wildcard freebsd/*)
OBJECTS := $(patsubst freebsd/%,man/%.md,$(SOURCES))

all: $(OBJECTS)

man/%.md: freebsd/%
	mkdir -p man
	groff -mandoc -Thtml < $< | pandoc -f html -t gfm -o $@
