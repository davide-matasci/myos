# The std demo programs: `thread::sleep` blocks the task for the duration
# (the program checks the wall clock); files are created, written, moved
# and removed, and refusals say why; the others run in heap's std stage.
t std_sleep /bin/std/sleep
t std_fs /bin/std/fs
