# The C++ runtimes against mlibc (smoke.cpp): exceptions through frames
# and across threads, RTTI, a global's constructor, threads and
# thread_locals, atomic waits, the containers, iostream and std::format,
# <filesystem>, <chrono> and <random>.
t -t mlibc,cxx cxx /bin/etc/mlibc/cxx_smoke
