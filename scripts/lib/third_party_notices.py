"""The licence texts of everything a package ships, as one text file (RD-1480-12).

`scripts/licenses.sh` calls `render` with the components it lists and writes the answer to
crates/rd-api/licenses/THIRD-PARTY-NOTICES.txt. Nothing here asks a network: a crate's texts are
read from its unpacked sources in cargo's registry, an npm package's from web/node_modules, the
Rust standard library's from the toolchain's own documentation, and the standard texts from
crates/rd-api/licenses/texts/. Equal inputs give equal bytes: components and texts are sorted, and
a text is named after its content, so an update moves only the lines it changes.

A component whose package carries no licence file is never given an invented copyright line. It
gets the standard text of every licence its SPDX expression names, and the authors its own manifest
names, marked as what they are.
"""

import dataclasses
import hashlib
import html
import os
import pathlib
import re
import textwrap

# A licence, notice or copyright file by its name; `third` is a package's own list of what it took in.
LICENCE_FILE = re.compile(r"^(licen[cs]es?|copying|copyright|notice|unlicen[cs]e|patents|ofl|third[-_]?party)",
                          re.IGNORECASE)
# Source files a name alone makes look like one (`license.rs`, `notice.js`).
SOURCE_FILE = re.compile(r"\.(rs|py|go|toml|json|[cm]?js|[cm]?ts|map|css|html|png|svg)$", re.IGNORECASE)
# Nothing under these is compiled in.
SKIPPED_DIRECTORIES = {".git", ".github", "benches", "docs", "examples", "fuzz", "node_modules", "test",
                       "testdata", "tests"}
IDENTIFIER = re.compile(r"[A-Za-z0-9.+-]+")
OPERATORS = {"AND", "OR", "WITH"}


@dataclasses.dataclass
class Component:
    name: str
    version: str
    license: str
    source: str
    directory: pathlib.Path | None = None
    # Files named by hand, beside the ones found in `directory` (the manifest's `license-file`).
    files: tuple = ()
    authors: tuple = ()
    # Texts that come from no file of their own: (name, text).
    texts: tuple = ()


def normalise(text):
    """The text as it is printed: LF, no trailing blanks, no control characters, no blank edges."""
    text = text.lstrip("﻿").replace("\r\n", "\n").replace("\r", "\n").replace("\f", "\n")
    text = "".join(char for char in text if char in "\n\t" or ord(char) >= 32)
    lines = [line.rstrip() for line in text.split("\n")]
    # git reads a line of seven `=`, `<`, `>` or `|` as a conflict marker left behind.
    lines = [line + line[0] if re.fullmatch(r"([=<>|])\1{6}", line) else line for line in lines]
    return "\n".join(lines).strip("\n") + "\n"


def read_text(path):
    data = path.read_bytes()
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("latin-1")


def digest(text):
    """Eight characters that name a text; spacing and line breaks do not tell two texts apart."""
    return hashlib.sha256(" ".join(text.split()).encode()).hexdigest()[:8]


def licence_files(directory):
    """Every licence file below a package's directory, sorted by its path."""
    found = []
    for folder, children, names in os.walk(directory):
        children[:] = sorted(child for child in children if child not in SKIPPED_DIRECTORIES)
        for name in names:
            if LICENCE_FILE.match(name) and not SOURCE_FILE.search(name):
                found.append(pathlib.Path(folder) / name)
            elif pathlib.Path(folder).name.upper() == "LICENSES" and name.endswith(".txt"):
                found.append(pathlib.Path(folder) / name)
    return sorted(set(found))


def identifiers(expression):
    """The licence and exception identifiers of an SPDX expression, in their order, each once."""
    words = IDENTIFIER.findall(expression.replace("/", " OR "))
    return list(dict.fromkeys(word for word in words if word not in OPERATORS))


class Texts:
    """The texts printed so far, by digest, and the standard texts they may fall back to."""

    def __init__(self, standard):
        self.standard = pathlib.Path(standard)
        self.printed = {}
        self.missing = set()

    def add(self, label, text):
        text = normalise(text)
        if not text.strip():
            return None
        key = digest(text)
        previous = self.printed.setdefault(key, (label, text))
        # One text under two names keeps the first by name, so the order of reading says nothing.
        if label < previous[0]:
            self.printed[key] = (label, text)
        return key

    def add_standard(self, identifier):
        path = self.standard / f"{identifier}.txt"
        if not path.is_file():
            self.missing.add(identifier)
            return None
        return self.add(f"standard text of {identifier}", read_text(path))


def component_lines(component, texts):
    """The entry of one component: what it is, where its source lies, which texts apply."""
    lines = [f"{component.name} {component.version} ({component.license})", f"    source: {component.source}"]
    references = []
    files = list(component.files)
    if component.directory is not None:
        files += licence_files(component.directory)
    for path in dict.fromkeys(files):
        name = path.name if component.directory is None else os.path.relpath(path, component.directory)
        name = name.replace(os.sep, "/")
        key = texts.add(f"{name} of {component.name}", read_text(path))
        if key:
            references.append(f"[{key}] {name}")
    for name, text in component.texts:
        key = texts.add(f"{name} of {component.name}", text)
        if key:
            references.append(f"[{key}] {name}")
    if references:
        lines += [f"    text:   {reference}" for reference in dict.fromkeys(references)]
        return lines
    lines.append("    The package carries no licence file; the standard text of what it declares applies.")
    if component.authors:
        lines.append(f"    Authors its manifest names: {', '.join(component.authors)}")
    for identifier in identifiers(component.license):
        key = texts.add_standard(identifier)
        if key:
            lines.append(f"    text:   [{key}] {identifier}")
    return lines


def rust_standard_library(page, version, texts_directory):
    """The standard library's components, from COPYRIGHT-library.html of the installed toolchain."""
    in_tree, _, out_of_tree = page.partition('<h2 id="out-of-tree-dependencies">')
    if not out_of_tree:
        raise ValueError("COPYRIGHT-library.html has no section on out-of-tree dependencies")
    summary = in_tree.partition('<h2 id="short-version">')[2]
    summary = re.sub(r"<[^>]+>", "", summary.partition(">")[2])
    summary = re.sub(r"\n\s*\n+", "\n", html.unescape(summary))
    applicable = []
    for identifier in ("MIT", "Apache-2.0", "LLVM-exception", "BSD-2-Clause", "Unicode-3.0"):
        path = pathlib.Path(texts_directory) / f"{identifier}.txt"
        applicable.append((f"{identifier}", read_text(path)))
    components = [Component(
        "Rust standard library and runtime", version, "MIT OR Apache-2.0",
        f"https://github.com/rust-lang/rust/tree/{version}",
        texts=(("COPYRIGHT-library.html (in-tree files)", summary), *applicable),
    )]
    for block in out_of_tree.split("<h3>")[1:]:
        title = html.unescape(block.partition("</h3>")[0]).split()[-1]
        name, _, number = re.match(r"(.+?)(-)(\d.*)$", title).groups()
        licence = re.search(r"<b>License:</b>\s*([^<]*)", block)
        notices = re.findall(r"<summary><code>([^<]*)</code></summary>\s*<pre>(.*?)</pre>", block, re.DOTALL)
        authors = re.search(r"<b>Authors:</b>\s*([^<]*)", block)
        components.append(Component(
            name, number, html.unescape(licence.group(1)).strip() if licence else "",
            f"https://crates.io/crates/{name}/{number}",
            authors=tuple(filter(None, [html.unescape(authors.group(1)).strip()])) if authors else (),
            texts=tuple((label, html.unescape(text)) for label, text in notices),
        ))
    return components


def render(preamble, sections, standard):
    """The whole file. `sections` is a list of (title, introduction, components)."""
    texts = Texts(standard)
    out = [normalise(preamble)]
    for title, introduction, components in sections:
        out.append(f"\n{'=' * 98}\n{title}\n{'=' * 98}\n")
        out.append(textwrap.fill(introduction, 98) + "\n")
        for component in sorted(components, key=lambda entry: (entry.name, entry.version)):
            out.append("\n" + "\n".join(component_lines(component, texts)) + "\n")
    if texts.missing:
        raise LookupError(sorted(texts.missing))
    out.append(f"\n{'=' * 98}\nThe texts\n{'=' * 98}\n")
    for key in sorted(texts.printed):
        label, text = texts.printed[key]
        out.append(f"\n{'-' * 98}\n[{key}] {label}\n{'-' * 98}\n\n{text}")
    return "".join(out)
