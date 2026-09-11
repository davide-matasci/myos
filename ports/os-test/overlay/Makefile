# os-test for myos — GNU-make harness driving tcc against the newlib sysroot.
# Upstream suite Makefiles are BSD-make; this replaces them. Run on the guest:
#
#   cd /lib/os-test
#   make                # compile + run every test; outcome -> out/<suite>/<test>.out
#   make SUITES=malloc  # restrict to a suite subset
#   make report         # pass/fail/compile_error summary
#
# Suite selection lives in SUITES; see ports/os-test/README.md.
SUITES := basic limits io malloc paths process signal stdio

CC = /bin/tcc/tcc
CFLAGS = -D_GNU_SOURCE -D_BSD_SOURCE -D_ALL_SOURCE -D_DEFAULT_SOURCE

SOURCES := $(shell find $(SUITES) -name '*.c' | sed 's,^\./,,' | sort)
OUTS := $(patsubst %.c,out/%.out,$(SOURCES))

.PHONY: all test report clean debug
all: test

debug:
	@echo SUITES=$(SUITES)
	@echo sources=$(words $(SOURCES))
	@echo outs=$(words $(OUTS))

test: $(OUTS)
	@sh misc/myos-report.sh

report: $(OUTS)
	@sh misc/myos-report.sh

out/%.out: %.c
	@sh misc/myos-run.sh '$(CC)' '$(CFLAGS)' '$<' '$(patsubst out/%.out,%,$@)'

clean:
	rm -rf out
