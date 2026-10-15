# The std demo programs: `thread::sleep` blocks the task for the duration
# (the program checks the wall clock and `Instant`); files are created,
# written, moved and removed, and refusals say why; threads share memory
# through the locks, keep their own thread-locals, and free their stacks and
# task slots joined or detached; the heap reuses what is freed (1 GiB through
# it in a process that stays small); the others run in heap's std stage.
t -t smoke std_sleep /bin/std/sleep
t -t smoke,fs std_fs /bin/std/fs
t -t smoke,sched std_thread /bin/std/thread
t -t smoke,mm std_alloc /bin/std/alloc
