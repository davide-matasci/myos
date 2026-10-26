# Sound (docs/sound.md): the test boots' virtio-sound cards and USB
# speaker, through the sound core's /dev/sound and the sound server, sndd.
# The first card plays into a WAV file the launcher reads once QEMU is gone
# (src/sound.rs): it must carry what sound_play and sound_mix played. The
# second records QEMU's silent input. The speaker plays into a WAV file of
# its own, which must carry sound_usb's tone.

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
# playback stream, the second's playback and capture streams, the
# speaker's playback stream, their ctl naming the card and what the stream
# can do.
sound_devices() {
	sound_wait || return 1
	ls /dev/sound > /tmp/sound.ls || return 1
	[ "$(grep -c . /tmp/sound.ls)" -ge 4 ] || { cat /tmp/sound.ls; return 1; }
	for n in $(cat /tmp/sound.ls); do
		grep -Eq '^card (virtio-sound|usb-audio) ' /dev/sound/$n/ctl &&
			grep -Eq '^direction (playback|capture)$' /dev/sound/$n/ctl &&
			grep -q '^formats.* s16' /dev/sound/$n/ctl &&
			grep -q '^rates.* 48000' /dev/sound/$n/ctl &&
			grep -q '^state stopped$' /dev/sound/$n/ctl || { cat /dev/sound/$n/ctl; return 1; }
	done
	grep -q '^0 playback virtio-sound ' /tmp/sound.list &&
		grep -q ' capture virtio-sound ' /tmp/sound.list &&
		grep -q ' playback usb-audio ' /tmp/sound.list || { cat /tmp/sound.list; return 1; }
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
# WAV file: about 32000 frames, and silent (QEMU's none input). QEMU's
# input runs on the host's clock, rec on the guest's, which TCG on a
# loaded host slows: up to twice as many frames come (a conversion left
# out would be six times as many).
sound_rec() {
	sound_wait || return 1
	/bin/etc/rec -r 16000 -c 1 -d 2 /tmp/rec.wav || return 1
	size=$(wc -c < /tmp/rec.wav)
	frames=$(( (size - 44) / 2 ))
	[ $frames -ge 24000 ] && [ $frames -le 64000 ] || { echo "$frames frames"; return 1; }
	# Bytes 44 on: zeros only.
	[ -z "$(tail -c +45 /tmp/rec.wav | tr -d '\000' | head -c 1)" ] || { echo "not silent"; return 1; }
}
t -t sound sound_rec sound_rec

# The USB speaker (usb_audio, docs/usb.md): 880 Hz for a second and a half
# through the server on its stream, isochronous packets on the xHCI, which
# the launcher looks for in the speaker's WAV file.
sound_usb() {
	sound_wait || return 1
	n=$(grep ' playback usb-audio ' /tmp/sound.list | cut -d' ' -f1)
	[ -n "$n" ] || { cat /tmp/sound.list; return 1; }
	/bin/etc/play -D $n -t 880 -d 1.5 &
	pid=$!
	sleep 1
	grep -q '^state running 48000 2 s16 ' /dev/sound/$n/ctl || { cat /dev/sound/$n/ctl; kill $pid; return 1; }
	wait $pid
}
t -t sound,usb sound_usb sound_usb
