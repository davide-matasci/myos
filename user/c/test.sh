# The C smokes: ptys (openpty round trip through the line discipline, EIO
# once the slave closes) and /dev/urandom (non-zero, distinct, changing).
t pty /bin/etc/pty_smoke 2
t urandom /bin/etc/urandom_smoke
