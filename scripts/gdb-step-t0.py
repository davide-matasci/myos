import gdb

gdb.execute("set pagination off")
gdb.execute("set confirm off")
gdb.execute("file /root/wt-stall/target/dropbear-riscv64-unknown-none")
gdb.execute("target remote localhost:1234")
# the `add t0,a6,t1` that computes the copy-loop bound
gdb.execute("break *0x400491c2")
gdb.execute("condition 1 $a6!=0")
print("ATTACHED watching conv_path add t0,a6,t1", flush=True)

def reg(r):
    return int(gdb.parse_and_eval("$" + r)) & 0xFFFFFFFFFFFFFFFF

while True:
    try:
        gdb.execute("continue")
    except gdb.error as e:
        print("GDB-EXIT:", e, flush=True)
        break
    try:
        pc = reg("pc")
        a6, t1, a5 = reg("a6"), reg("t1"), reg("a5")
        # step over: add (1), beqz (1 if not taken), mv (1) -> land at 0x491cc
        for i in range(3):
            gdb.execute("stepi")
            p = reg("pc")
            t0 = reg("t0")
            print("STEP%d pc=%x t0=%x (a6=%x t1=%x expected=%x)" % (i, p, t0, a6, t1, (a6 + t1) & 0xFFFFFFFFFFFFFFFF), flush=True)
            if p == 0x400491CC and t0 == 0:
                print("ANOMALY: t0 zeroed between add and loop top", flush=True)
                print("in-step pc=%x sstatus-seen-below" % p, flush=True)
                for r in ("ra", "sp", "a5", "a6", "t0", "t1", "a7", "t6", "tp", "scause", "sepc", "scratch"):
                    try:
                        print("  %s=%x" % (r, reg(r)), flush=True)
                    except gdb.error:
                        pass
                break
            if p != (0x400491C6, 0x400491CA, 0x400491CC)[i]:
                print("UNEXPECTED pc path at step %d: %x" % (i, p), flush=True)
                break
    except gdb.error as e:
        print("READ-ERR", e, flush=True)
        break
