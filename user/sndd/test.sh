# Sound (docs/sound.md): the test boots' virtio-sound cards, through the
# sound core's /dev/sound and the sound server, sndd. The first card plays
# into a WAV file the launcher reads once QEMU is gone (src/sound.rs): it
# must carry what sound_play and sound_mix played. The second records
# QEMU's silent input.

# The server answers once it announced its socket (init starts it).
sound_wait() {
	for i in 1 2 3 4 5 6 7 8 9 10; do
		/bin/etc/play -l > /tmp/sound.list 2>&1 && grep -q playback /tmp/sound.list && return 0
		sleep 1
	done
	cat /tmp/sound.list
	return 1
}

# /dev/sound's streams, and the server's list of them: the first card's
# playback stream, the second's playback and capture streams, their ctl
# naming the card and what the stream can do.
sound_devices() {
	sound_wait || return 1
	ls /dev/sound > /tmp/sound.ls || return 1
	[ "$(grep -c . /tmp/sound.ls)" -ge 3 ] || { cat /tmp/sound.ls; return 1; }
	for n in $(cat /tmp/sound.ls); do
		grep -q '^card virtio-sound ' /dev/sound/$n/ctl &&
			grep -Eq '^direction (playback|capture)$' /dev/sound/$n/ctl &&
			grep -q '^formats.* s16' /dev/sound/$n/ctl &&
			grep -q '^rates.* 48000' /dev/sound/$n/ctl &&
			grep -q '^state stopped$' /dev/sound/$n/ctl || { cat /dev/sound/$n/ctl; return 1; }
	done
	grep -q '^0 playback virtio-sound ' /tmp/sound.list &&
		grep -q ' capture virtio-sound ' /tmp/sound.list || { cat /tmp/sound.list; return 1; }
}
t -t sound sound_devices sound_devices

# A tone through the server: 440 Hz for a second and a half on the first
# card, which the launcher looks for in its WAV file. While it plays, the
# stream runs at the server's 48 kHz stereo s16; it stops once nobody
# plays.
sound_play() {
	sound_wait || return 1
	/bin/etc/play -t 440 -d 1.5 &
	pid=$!
	sleep 1
	grep -q '^state running 48000 2 s16 ' /dev/sound/0/ctl || { cat /dev/sound/0/ctl; kill $pid; return 1; }
	wait $pid || return 1
	for i in 1 2 3 4 5; do
		grep -q '^state stopped$' /dev/sound/0/ctl && return 0
		sleep 1
	done
	cat /dev/sound/0/ctl
	return 1
}
t -t sound sound_play sound_play

# Two programs at once, mixed: 440 Hz at 48 kHz stereo and 660 Hz at
# 22050 Hz mono u8 (converted to the stream's), which the launcher finds
# together in the WAV file.
sound_mix() {
	sound_wait || return 1
	/bin/etc/play -t 440 -d 1.5 &
	a=$!
	/bin/etc/play -r 22050 -c 1 -f u8 -t 660 -d 1.5 || { kill $a; return 1; }
	wait $a
}
t -t sound sound_mix sound_mix

# Recording: the second card's capture stream through the server, two
# seconds of 16 kHz mono (the server converts its 48 kHz stereo) into a
# WAV file: about 32000 frames (TCG's clock is loose: a quarter either
# way), and silent (QEMU's none input).
sound_rec() {
	sound_wait || return 1
	/bin/etc/rec -r 16000 -c 1 -d 2 /tmp/rec.wav || return 1
	size=$(wc -c < /tmp/rec.wav)
	frames=$(( (size - 44) / 2 ))
	[ $frames -ge 24000 ] && [ $frames -le 40000 ] || { echo "$frames frames"; return 1; }
	# Bytes 44 on: zeros only.
	[ -z "$(tail -c +45 /tmp/rec.wav | tr -d '\000' | head -c 1)" ] || { echo "not silent"; return 1; }
}
t -t sound sound_rec sound_rec
