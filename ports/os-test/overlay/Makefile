# os-test for myos — GNU-make harness driving tcc against the newlib sysroot.
# Upstream suite Makefiles are BSD-make; this replaces them. Run on the guest:
#
#   cd /lib/os-test
#   make                # compile + run every test; outcome -> out/<suite>/<test>.out
#   make SUITES=malloc  # restrict to a suite subset
#   make SUITES=basic TESTS="pwd/setpwent ctype/isalpha" report
#   make SUITES=basic TESTLIST=misc/ci-basic-smoke.tests report
#   make report         # pass/fail/compile_error summary
#
# Suite selection lives in SUITES; optional TESTS / AREAS / TESTLIST narrow
# which sources under those suites are built. See ports/os-test/README.md.
SUITES := basic limits io malloc paths process signal stdio

CC = /bin/tcc/tcc
CFLAGS = -D_GNU_SOURCE -D_BSD_SOURCE -D_ALL_SOURCE -D_DEFAULT_SOURCE

# Note: no $(shell ...) — make's $(shell) forks a pipe reader and hangs on
# myos (pipe EOF never reaches the reader). $(wildcard) / `include` are pure
# make, no fork. Depth: suite/<dir>/<test>.c and suite/<test>.c both allowed
# when TESTS/AREAS/TESTLIST are unset.

# TESTLIST: makefile fragment with `TESTS += path` lines (path relative to
# each SUITES entry, no .c). Prefer this for boot CI — short typed command.
ifdef TESTLIST
include $(TESTLIST)
endif

comma := ,
empty :=
space := $(empty) $(empty)

ifdef TESTS
# Explicit list: paths relative to each suite (e.g. pwd/setpwent with
# SUITES=basic → basic/pwd/setpwent.c). Missing files are skipped.
SOURCES := $(sort $(foreach t,$(TESTS),$(foreach s,$(SUITES),$(wildcard $(s)/$(t).c))))
else ifdef AREAS
# Subdir filter: AREAS=pwd,grp,ctype or AREAS="pwd grp ctype"
AREAS_LIST := $(subst $(comma),$(space),$(AREAS))
SOURCES := $(sort $(foreach a,$(AREAS_LIST),$(foreach s,$(SUITES),$(wildcard $(s)/$(a)/*.c))))
else
SOURCES := $(sort $(foreach s,$(SUITES),$(wildcard $(s)/*.c $(s)/*/*.c)))
endif
OUTS := $(patsubst %.c,out/%.out,$(SOURCES))

.PHONY: all test report clean debug
all: test

debug:
	@echo SUITES=$(SUITES)
	@echo TESTS=$(TESTS)
	@echo AREAS=$(AREAS)
	@echo TESTLIST=$(TESTLIST)
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
