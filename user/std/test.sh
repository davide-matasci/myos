# The std demo programs: `thread::sleep` blocks the task for the duration
# (the program checks the wall clock and `Instant`); files are created,
# written, moved and removed, and refusals say why; threads share memory
# through the locks, keep their own thread-locals, and free their stacks and
# task slots joined or detached; the others run in heap's std stage.
t std_sleep /bin/std/sleep
t std_fs /bin/std/fs
t std_thread /bin/std/thread
