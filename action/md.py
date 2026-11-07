#!/usr/bin/env python3
"""Markdown escaping for the action's own output (ADR 0014 section 9).

  md.py fence  <file>          print the file as a code block whose fence is
                               longer than any backtick run inside it
  md.py inline <text> [max]    one line of text, safe inside Markdown/HTML
                               (no formatting, links, HTML or @-mentions)
"""
import re
import sys


def fence(text: str) -> str:
    longest = max((len(m) for m in re.findall(r"`+", text)), default=0)
    f = "`" * max(3, longest + 1)
    if not text.endswith("\n"):
        text += "\n"
    return f"{f}text\n{text}{f}\n"


def inline(text: str, limit: int = 200) -> str:
    text = " ".join(text.split())  # one line, no indentation tricks
    if len(text) > limit:
        text = text[: limit - 1] + "…"
    out = []
    for ch in text:
        if ch == "&":
            out.append("&amp;")
        elif ch == "<":
            out.append("&lt;")
        elif ch == ">":
            out.append("&gt;")
        elif ch == "@":
            out.append("&#64;")  # no mentions
        elif ch in "\\`*_{}[]()#+-.!|~":
            out.append("\\" + ch)
        else:
            out.append(ch)
    return "".join(out)


def main(argv):
    if len(argv) >= 3 and argv[1] == "fence":
        with open(argv[2], encoding="utf-8", errors="replace") as fh:
            sys.stdout.write(fence(fh.read()))
    elif len(argv) >= 3 and argv[1] == "inline":
        limit = int(argv[3]) if len(argv) > 3 else 200
        sys.stdout.write(inline(argv[2], limit))
    elif len(argv) >= 2 and argv[1] == "selftest":
        assert fence("a ``` b\n").startswith("````text\n")
        assert fence("x").endswith("x\n```\n")
        assert fence("`````") .count("``````") == 2
        s = inline("Fix *bold* [x](http://e) <img src=x> @org/team ```\nnext")
        assert "<" not in s and "@" not in s and "\n" not in s and "](" not in s
        assert "\\*bold\\*" in s and "\\`\\`\\`" in s
        print("md.py: ok")
    else:
        sys.stderr.write(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
