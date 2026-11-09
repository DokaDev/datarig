#!/usr/bin/env python3
"""Public text style check: no em dash, British spelling.

Usage: python3 scripts/text-style.py [--counts]

Fails (exit 1) with file:line: message for each finding, run from the repository root.

1. No em dash (U+2014) in the README, CONTRIBUTING, SECURITY, the pull request template, docs/
   (the site with its generated pages and screens, the demo tapes), changes/, every catalog in
   crates/i18n/locales/, the Cargo.toml files, the insta snapshots, and the string literals of crates/
   (comments are not checked). Use a colon, a comma, parentheses or a new sentence; an en dash in
   a range (1–5) and `--` in a command line are fine.
2. British spelling in the prose of the README, CONTRIBUTING, SECURITY, the pull request
   template, docs/site/src, the generated docs/keybindings.md and docs/themes.md, changes/ and
   the values of the English catalog (crates/i18n/locales/en/*.toml): colour, behaviour, centre,
   favourite, cancelled, organise,
   analyse... Code is exempt: fenced code blocks, `inline code`, {placeholders}, URLs, words
   joined to `_`, `.`, `-`, `/` or `::`, and words in capitals (the SQL keyword ANALYZE).
   A name typed as it is outside code goes in ALLOW_PHRASES or ALLOW_KEYS, with the reason.

--counts prints the number of findings per rule and file instead.
"""

import os
import re
import subprocess
import sys

EM_DASH = "—"

# Exact US spellings and their British forms. The -ize/-yze endings are matched by a rule of
# their own below.
US_UK = {
    "color": "colour",
    "colors": "colours",
    "colored": "coloured",
    "coloring": "colouring",
    "colorful": "colourful",
    "behavior": "behaviour",
    "behaviors": "behaviours",
    "behavioral": "behavioural",
    "center": "centre",
    "centers": "centres",
    "centered": "centred",
    "favorite": "favourite",
    "favorites": "favourites",
    "favor": "favour",
    "flavor": "flavour",
    "honor": "honour",
    "neighbor": "neighbour",
    "neighbors": "neighbours",
    "gray": "grey",
    "grayed": "greyed",
    "canceled": "cancelled",
    "canceling": "cancelling",
    "cancelation": "cancellation",
    "labeled": "labelled",
    "labeling": "labelling",
    "modeled": "modelled",
    "modeling": "modelling",
    "traveled": "travelled",
    "signaled": "signalled",
    "signaling": "signalling",
    "tunneled": "tunnelled",
    "tunneling": "tunnelling",
    "totaled": "totalled",
    "leveled": "levelled",
    "marshaled": "marshalled",
    "fulfill": "fulfil",
    "fulfills": "fulfils",
    "defense": "defence",
    "offense": "offence",
    "artifact": "artefact",
    "artifacts": "artefacts",
    "fiber": "fibre",
    "acknowledgment": "acknowledgement",
    "toward": "towards",
    "afterward": "afterwards",
}

# -ize / -yze words that are spelt the same in British English.
IZE_SAME = {"size", "sizes", "sized", "sizing", "seize", "seizes", "seized", "seizing", "prize",
            "prizes", "capsize", "capsized"}

# Phrases outside code that keep a US spelling, with the reason. They are blanked like code.
ALLOW_PHRASES = [
    # The argument of the `:explain` command, typed as it is (also accepted as `analyse`).
    ":explain analyze",
]

# Catalog keys whose value is a name typed as it is, not prose.
ALLOW_KEYS = {
    "commands.arg.analyze",  # the `:explain` argument
}

IZE_RE = re.compile(r"^([a-z]+)(i|y)z(e|es|ed|ing|ation|ations|er|ers)$")
WORD_RE = re.compile(r"(?<![\w./:\-])([A-Za-z]+)(?![\w/:\-]|\.\w|::)")
URL_RE = re.compile(r"<?https?://\S+")
CODE_SPAN_RE = re.compile(r"(`+)(.+?)\1")
PLACEHOLDER_RE = re.compile(r"\{[^{}]*\}")
TABLE_RE = re.compile(r"\[[a-z_.]+\]")  # a TOML table name such as [colors]


def git_files(*pathspecs):
    out = subprocess.run(["git", "ls-files", "-z", "--", *pathspecs], check=True,
                         capture_output=True).stdout.decode()
    return [f for f in out.split("\0") if f]


def us_word(word):
    """The British spelling of `word` when it is a US one, else None."""
    if word.isupper() and len(word) > 1:
        return None
    low = word.lower()
    if low in US_UK:
        uk = US_UK[low]
    else:
        m = IZE_RE.match(low)
        if not m or low in IZE_SAME or low.endswith("size") or low.endswith("sized") \
                or low.endswith("sizes") or low.endswith("sizing"):
            return None
        uk = f"{m.group(1)}{m.group(2)}s{m.group(3)}"
    return uk[0].upper() + uk[1:] if word[0].isupper() else uk


def prose_findings(text):
    """(column, word, uk) for each US spelling in a line of prose, code removed."""
    text = URL_RE.sub(lambda m: " " * len(m.group(0)), text)
    text = CODE_SPAN_RE.sub(lambda m: " " * len(m.group(0)), text)
    text = PLACEHOLDER_RE.sub(lambda m: " " * len(m.group(0)), text)
    text = TABLE_RE.sub(lambda m: " " * len(m.group(0)), text)
    for phrase in ALLOW_PHRASES:
        text = text.replace(phrase, " " * len(phrase))
    for m in WORD_RE.finditer(text):
        uk = us_word(m.group(1))
        if uk:
            yield m.start(1), m.group(1), uk


def markdown_lines(path):
    """(line number, text) of the prose lines of a Markdown file: fenced blocks left out."""
    fence = None
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            s = line.lstrip()
            if fence:
                if s.startswith(fence):
                    fence = None
                continue
            if s.startswith("```") or s.startswith("~~~"):
                fence = s[:3]
                continue
            # Indented code and HTML comments of templates are not prose either.
            if line.startswith("    ") and not s.startswith(("-", "*", "|")) and fence is None \
                    and line.strip() and not line[4:5].isalpha():
                continue
            yield n, line.rstrip("\n")


def toml_values(path):
    """(line number, text) of the string values of a catalog: keys and comments left out."""
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            s = line.strip()
            if not s or s.startswith("#") or "=" not in s:
                continue
            key, value = s.split("=", 1)
            if key.strip().strip('"') in ALLOW_KEYS:
                continue
            yield n, " ".join(re.findall(r'"((?:[^"\\]|\\.)*)"', value))


def rust_literal_dashes(path):
    """Line numbers of em dashes inside string or char literals of a Rust file."""
    with open(path, encoding="utf-8") as f:
        src = f.read()
    if EM_DASH not in src and "\\u{2014}" not in src:
        return []
    found = []
    i, n, line = 0, len(src), 1
    state = None  # None, "str", "raw", "block"
    raw_end = ""
    depth = 0
    while i < n:
        c = src[i]
        if c == "\n":
            line += 1
        if state is None:
            if src.startswith("//", i):
                j = src.find("\n", i)
                i = n if j < 0 else j
                continue
            if src.startswith("/*", i):
                state, depth = "block", 1
                i += 2
                continue
            m = re.compile(r'b?r(#*)"').match(src, i)
            if m and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
                state, raw_end = "raw", '"' + m.group(1)
                i = m.end()
                continue
            if c == '"':
                state = "str"
                i += 1
                continue
            m = re.compile(r"'(\\.[^']*|[^'\\])'").match(src, i)
            if m:
                if EM_DASH in m.group(0):
                    found.append(line)
                i = m.end()
                continue
        elif state == "block":
            if src.startswith("/*", i):
                depth += 1
                i += 2
                continue
            if src.startswith("*/", i):
                depth -= 1
                i += 2
                if depth == 0:
                    state = None
                continue
        elif state == "str":
            if src.startswith("\\u{2014}", i):
                found.append(line)
            if c == "\\":
                if src[i + 1:i + 2] == "\n":
                    line += 1
                i += 2
                continue
            if c == '"':
                state = None
            elif c == EM_DASH:
                found.append(line)
        elif state == "raw":
            if src.startswith(raw_end, i):
                state = None
                i += len(raw_end)
                continue
            if c == EM_DASH:
                found.append(line)
        i += 1
    return found


def main():
    counts = "--counts" in sys.argv[1:]
    findings = []

    def add(rule, path, line, msg):
        findings.append((rule, path, line, msg))

    text_files = git_files("README.md", "CONTRIBUTING.md", "SECURITY.md",
                           ".github/pull_request_template.md", "docs", "changes", "crates/i18n/locales",
                           "Cargo.toml", "crates/*/Cargo.toml", "crates/*.snap")
    for path in text_files:
        if path.endswith((".png", ".gif", ".jpg", ".svg", ".ico", ".woff", ".woff2")):
            continue
        try:
            with open(path, encoding="utf-8") as f:
                for n, line in enumerate(f, 1):
                    if EM_DASH in line:
                        add("em-dash", path, n, "em dash: use a colon, a comma, parentheses "
                            "or a new sentence")
        except UnicodeDecodeError:
            continue

    for path in git_files("crates/*.rs"):
        for n in rust_literal_dashes(path):
            add("em-dash", path, n, "em dash in a string literal")

    prose = git_files("README.md", "CONTRIBUTING.md", "SECURITY.md",
                      ".github/pull_request_template.md", "docs/site/src/*.md",
                      "docs/keybindings.md", "docs/themes.md", "changes/*.md")
    for path in prose:
        for n, line in markdown_lines(path):
            for _col, word, uk in prose_findings(line):
                add("spelling", path, n, f"US spelling '{word}': write '{uk}'")
    for path in git_files("crates/i18n/locales/en/*.toml"):
        for n, line in toml_values(path):
            for _col, word, uk in prose_findings(line):
                add("spelling", path, n, f"US spelling '{word}': write '{uk}'")

    if counts:
        per = {}
        for rule, path, _n, msg in findings:
            per[(rule, path)] = per.get((rule, path), 0) + 1
        for (rule, path), c in sorted(per.items()):
            print(f"{c:5} {rule:9} {path}")
        words = {}
        for rule, _p, _n, msg in findings:
            if rule == "spelling":
                w = msg.split("'")[1].lower()
                words[w] = words.get(w, 0) + 1
        print(" ".join(f"{w}:{c}" for w, c in sorted(words.items(), key=lambda x: -x[1])))
        print(f"total em-dash {sum(1 for f in findings if f[0] == 'em-dash')}, "
              f"spelling {sum(1 for f in findings if f[0] == 'spelling')}")
        return 0
    for _rule, path, n, msg in findings:
        print(f"{path}:{n}: {msg}")
    if findings:
        print(f"\n{len(findings)} public text style finding(s); see CONTRIBUTING.md, "
              "\"Public text style\".", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    os.chdir(subprocess.run(["git", "rev-parse", "--show-toplevel"], check=True,
                            capture_output=True, text=True).stdout.strip())
    sys.exit(main())
