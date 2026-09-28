import gdb, time

gdb.execute("set pagination off")
gdb.execute("set confirm off")
# riscv64 remote over QEMU gdbstub
gdb.execute("file /root/wt-stall/target/dropbear-riscv64-unknown-none")
gdb.execute("target remote localhost:1234")
# conv_path first-copy loop top: anomaly = t0 (loop bound) == 0 while out != 0
gdb.execute("break *0x400491cc")
gdb.execute("condition 1 $t0==0 && $a6!=0")
print("ATTACHED, watching conv_path t0==0 anomaly", flush=True)
while True:
    try:
        gdb.execute("continue")
    except gdb.error as e:
        print("GDB-EXIT:", e, flush=True)
        break
    try:
        vals = {}
        for r in ("pc", "ra", "sp", "a5", "a6", "t0", "t1", "a7", "t6", "a0", "a1"):
            vals[r] = int(gdb.parse_and_eval("$" + r)) & 0xFFFFFFFFFFFFFFFF
        print("T0ZERO pc=%(pc)x ra=%(ra)x sp=%(sp)x a5=%(a5)x a6=%(a6)x t0=%(t0)x t1=%(t1)x a7=%(a7)x t6=%(t6)x a0=%(a0)x a1=%(a1)x" % vals, flush=True)
        gdb.execute("x/6i $pc-16")
    except gdb.error as e:
        print("READ-ERR", e, flush=True)
