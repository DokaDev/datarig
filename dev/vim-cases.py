#!/usr/bin/env python3
"""Expected results of vim commands, from Neovim, for the editor's table tests.

The editor's vim tests (crates/datarig-tui/src/widgets/editor/*/tests.rs) are tables of
cases whose expected text, cursor and register come from Neovim run on the same text with the
same keys. This script runs the cases in a headless Neovim (no user config) and prints the
table rows.

    python3 dev/vim-cases.py cases.json > rows.rs

cases.json is a list of cases:

    [text, row, col, keys]                       -> a `Case` row: (text, cursor, keys,
                                                    text after, cursor after, register after)
    [text, row, col, keys, {"lines": H, "top": T}] -> a view row, H lines on screen from line
                                                    T: (text, cursor, H, T, keys, cursor after,
                                                    top line after)

Rows and columns count from 0, columns in characters (not bytes). Keys use Vim's `:normal`
notation (`<Esc>`, `<CR>`, `<C-d>`). Hangul in the output is written as \\u{...} escapes.
The options match the editor: autoindent, 4-column indent written with spaces, no line wrap,
`startofline`, no `joinspaces`, `;` after `t` not staying next to its character.

A command that fails in Neovim ends `:normal` there, so a case whose keys fail before the last
one does not test the keys after it: keep the failing command last.
"""
import json
import os
import subprocess
import sys
import tempfile

OPTIONS = ("set nofixeol noeol autoindent sol backspace=indent,eol,start sw=4 ts=4 sts=0 et "
           "nojoinspaces so=0 nowrap cpo-=; nosmartindent")


def run(text, row, col, keys, lines=None, top=None):
    """Neovim's text, cursor (row, byte column), register and top line after `keys`."""
    with tempfile.TemporaryDirectory() as d:
        src, out, script = (os.path.join(d, n) for n in ("in.txt", "out.json", "run.vim"))
        with open(src, "w") as f:
            f.write(text)
        k = keys.replace("\\", "\\\\").replace("<", "\\<").replace('"', '\\"')
        cmds = [OPTIONS, "silent! nunmap Y"]
        if lines:
            cmds.append(f"set lines={lines + 2}")
        cmds.append(f"call cursor({row + 1}, {col + 1})")
        if top is not None:
            cmds.append(f"call winrestview({{'topline': {top + 1}}})")
        cmds += [
            f'exe "normal {k}"',
            "let p = getcurpos()",
            "call writefile([json_encode({'text': join(getline(1, '$'), \"\\n\"), 'row': p[1] - 1,"
            " 'col': p[2] - 1, 'reg': getreg('\"'), 'regtype': getregtype('\"'), 'top': line('w0') - 1,"
            f" 'mode': mode(), 'lines': winheight(0)}})], '{out}')",
            "qa!",
        ]
        with open(script, "w") as f:
            f.write("\n".join(cmds) + "\n")
        subprocess.run(["nvim", "--headless", "-u", "NONE", "-i", "NONE", "-n", src, "-S", script],
                       capture_output=True, timeout=20, check=False)
        with open(out) as f:
            o = json.load(f)
    if lines and o["lines"] != lines:
        sys.exit(f"{keys!r}: Neovim had {o['lines']} lines on screen, not {lines}")
    if o["mode"] != "n":
        print(f"{keys!r} on {text!r}: Neovim ended in mode {o['mode']!r}", file=sys.stderr)
    return o


def rust(s):
    """A Rust string literal, Hangul as \\u{...} escapes."""
    out = json.dumps(s, ensure_ascii=False)
    hangul = lambda c: 0x1100 <= ord(c) <= 0x11FF or 0x3130 <= ord(c) <= 0x318F or 0xAC00 <= ord(c) <= 0xD7A3
    return "".join(f"\\u{{{ord(c):X}}}" if hangul(c) else c for c in out)


def char_col(line, byte_col):
    return len(line.encode()[:byte_col].decode(errors="ignore"))


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with open(sys.argv[1]) as f:
        cases = json.load(f)
    for case in cases:
        text, row, col, keys = case[:4]
        view = case[4] if len(case) > 4 else None
        byte_col = len(text.split("\n")[row][:col].encode())
        o = run(text, row, byte_col, keys, *(view["lines"], view.get("top", 0)) if view else ())
        at = (o["row"], char_col(o["text"].split("\n")[o["row"]], o["col"]))
        if view:
            print(f"    ({rust(text)}, ({row}, {col}), {view['lines']}, {view.get('top', 0)}, {rust(keys)}, {at}, {o['top']}),")
            continue
        if o["regtype"] == "":
            reg = "None"
        elif o["regtype"] == "V":
            reg = f"Some(({rust(o['reg'].removesuffix(chr(10)))}, true))"
        else:
            reg = f"Some(({rust(o['reg'])}, false))"
        print(f"    ({rust(text)}, ({row}, {col}), {rust(keys)}, {rust(o['text'])}, {at}, {reg}),")


if __name__ == "__main__":
    main()
