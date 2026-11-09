"""ARIB 2バイト文字 (JIS X 0213 第1面 / 第2面) → Unicode 表を生成する（Python 標準ライブラリのみ）。
変換表は Python の codec から作る。優先順位: euc_jis_2004 → euc_jp → cp932。
出力: crates/core/src/arib/jis.bin = u32 LE × (2 面 × 94 × 94)。0 = 未定義、bit31 = 2 文字（PAIRS の添字）
      crates/core/src/arib/jis_pairs.rs = 2 文字になる符号の文字列表
ARIB の追加記号・追加漢字は crates/core/src/arib/addsym.rs に手書きで置いている（このスクリプトでは作らない）。

使い方: python3 tools/gen_jis.py"""
import struct, os
out, pairs = [], []
def dec(bs, encs):
    for e in encs:
        try:
            return bs.decode(e)
        except UnicodeDecodeError:
            pass
    return None
for plane in (1, 2):
    for c1 in range(0x21, 0x7F):
        for c2 in range(0x21, 0x7F):
            if plane == 1:
                s = dec(bytes([c1 | 0x80, c2 | 0x80]), ("euc_jis_2004", "euc_jp", "cp932"))
            else:
                s = dec(bytes([0x8F, c1 | 0x80, c2 | 0x80]), ("euc_jis_2004",))
            if not s:
                out.append(0)
            elif len(s) == 1:
                out.append(ord(s))
            else:
                assert len(s) == 2, s
                out.append(0x80000000 | len(pairs))
                pairs.append(s)
here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, "..", "crates", "core", "src", "arib", "jis.bin"), "wb") as f:
    f.write(struct.pack(f"<{len(out)}I", *out))
with open(os.path.join(here, "..", "crates", "core", "src", "arib", "jis_pairs.rs"), "w") as f:
    f.write("// tools/gen_jis.py で生成\npub const PAIRS: &[&str] = &[\n")
    for s in pairs:
        f.write("    \"" + "".join(f"\\u{{{ord(c):x}}}" for c in s) + "\",\n")
    f.write("];\n")
print(len(out), "entries,", len(pairs), "pairs")
