#!/usr/bin/env python3
# A stand-in for gperf, enough for fontconfig's src/fcobjshash.gperf (the
# build's only gperf input): the declarations and the struct are copied, and
# the keywords are looked up with a linear search instead of a perfect hash.
# fontconfig has about 60 of them, looked up by name a few times per pattern.
#
# usage: gperf-lite.py [gperf options] FILE > fcobjshash.h
import re
import sys

src = open([a for a in sys.argv[1:] if not a.startswith("-")][-1]).read()
# The input is the C preprocessor's output: what the compiler wrapper
# force-includes comes before gperf's first directive. Drop it.
src = src[re.search(r"^%", src, re.M).start():]
head, _, keywords = src.partition("\n%%\n")
keywords = keywords.split("\n%%")[0]

out = []
# %{ ... %}: verbatim C.
for block in re.findall(r"^%\{\n(.*?)^%\}", head, re.S | re.M):
    out.append(block)
# Everything else of the head that is not a % directive: the struct.
rest = re.sub(r"^%\{\n.*?^%\}\n", "", head, flags=re.S | re.M)
out.extend(line for line in rest.splitlines() if line and not line.startswith("%"))

words = []
for line in keywords.splitlines():
    line = line.strip()
    if not line or line.startswith("#"):
        continue
    name, _, value = line.partition(",")
    words.append((name.strip().strip('"'), value.strip()))

offsets, pool, pos = [], [], 0
for name, _ in words:
    offsets.append(pos)
    pool.append(name)
    pos += len(name) + 1

out.append("static const char FcObjectTypeNamePool_contents[] =")
out.extend('  "%s\\0"' % name for name in pool)
out.append(";")
out.append("#define FcObjectTypeNamePool ((const char *) FcObjectTypeNamePool_contents)")
out.append("""
static unsigned int
FcObjectTypeHash (register const char *str, register FC_GPERF_SIZE_T len)
{
  (void) str;
  return (unsigned int) len;
}
""")
out.append("static const struct FcObjectTypeInfo FcObjectTypeWords[] = {")
out.extend("  {%d, %s}," % (off, value) for off, (_, value) in zip(offsets, words))
out.append("};")
out.append("""
static const struct FcObjectTypeInfo *
FcObjectTypeLookup (register const char *str, register FC_GPERF_SIZE_T len)
{
  unsigned int i;
  (void) FcObjectTypeHash;
  for (i = 0; i < sizeof FcObjectTypeWords / sizeof FcObjectTypeWords[0]; i++)
    {
      const char *name = FcObjectTypeNamePool + FcObjectTypeWords[i].name;
      if (strlen (name) == len && memcmp (name, str, len) == 0)
        return &FcObjectTypeWords[i];
    }
  return 0;
}""")
print("\n".join(out))
