# myos Phase-1 local porcelain: no network helpers, no scripting runtimes.
# Sourced after config.mak.uname — override host Linux detections that do not
# apply to freestanding myos.
NO_GETTEXT = YesPlease
NO_ICONV = YesPlease
NO_PERL = YesPlease
NO_PYTHON = YesPlease
NO_TCLTK = YesPlease
NO_PTHREADS = YesPlease
NO_OPENSSL = YesPlease
NO_CURL = YesPlease
NO_EXPAT = YesPlease
NO_RUST = YesPlease
NO_GITWEB = YesPlease
NO_REGEX = YesPlease
NO_SYS_POLL_H = YesPlease
NO_UNIX_SOCKETS = YesPlease
NO_IPV6 = YesPlease
NO_NSEC = YesPlease
NO_PREAD = YesPlease
NO_GETPAGESIZE = YesPlease
SKIP_DASHED_BUILT_INS = YesPlease

# newlib has strlcpy; undo Linux uname's NO_STRLCPY.
NO_STRLCPY =

# Do not pull Linux-only host features into the guest binary.
HAVE_SYSINFO =
HAVE_PLATFORM_PROCINFO =
HAVE_LINUX_MAGIC_H =
HAVE_SYNC_FILE_RANGE =
PROCFS_EXECUTABLE_PATH =
COMPAT_OBJS =
EXTLIBS =
FSMONITOR_DAEMON_BACKEND =
FSMONITOR_OS_SETTINGS =
LINK_FUZZ_PROGRAMS =

# Use /dev/urandom path in wrapper.c; myos_stubs provides getrandom() fallback
# via a tiny software fill if we keep HAVE_GETRANDOM + sys/random.h instead.
# Prefer software getrandom stub (no /dev/urandom on myos yet).
CSPRNG_METHOD = getrandom

HAVE_ALLOCA_H = YesPlease
HAVE_PATHS_H = YesPlease
HAVE_DEV_TTY = YesPlease
HAVE_CLOCK_GETTIME = YesPlease
HAVE_CLOCK_MONOTONIC = YesPlease
HAVE_GETDELIM = YesPlease
FREAD_READS_DIRECTORIES = UnfortunatelyYes

DEFAULT_EDITOR = vim
DEFAULT_PAGER = cat
COMPUTE_HEADER_DEPENDENCIES = no
INSTALL_SYMLINKS =
