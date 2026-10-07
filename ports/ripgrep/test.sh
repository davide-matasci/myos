# ripgrep: a parallel search (-j4: the directory walker and the searchers
# on std::thread) finds the needle in every file of a tree that has it.
rg_threads() {
	rm -rf /tmp/rgt
	for d in a b c; do
		mkdir -p /tmp/rgt/$d || return 1
		for i in 1 2 3 4 5 6; do
			echo "x needle $d$i" > /tmp/rgt/$d/f$i
			echo "nothing here" > /tmp/rgt/$d/g$i
		done
	done
	n=$(/bin/coreutils/rg -j4 --no-config --no-mmap -l needle /tmp/rgt | /bin/sbase/wc -l)
	echo "files: $n"
	[ $n -eq 18 ] && rm -rf /tmp/rgt
}
t rg_threads rg_threads
