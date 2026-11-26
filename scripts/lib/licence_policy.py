"""Which licences a shipped dependency may have (RD-1480-12): the `allow` list of deny.toml's
`[licenses]`, the one cargo-deny holds the crates to, and what the npm packages need beyond it.
`scripts/licenses.sh` refuses to write a list with an entry outside it; docker/python-licences.py
does the same for the Python packages of the image, with its own list."""

import pathlib
import re
import tomllib

# The licences only npm packages carry: the fonts' and a permissive one of two small modules.
NPM_ONLY = {"BlueOak-1.0.0", "OFL-1.1"}


def allowed_licences(deny="deny.toml"):
    """The identifiers deny.toml allows, `X WITH Y` as one."""
    return set(tomllib.loads(pathlib.Path(deny).read_text())["licenses"]["allow"])


def is_allowed(expression, allowed):
    """Whether an SPDX expression can be met with the allowed licences alone: every part of an
    AND, one of an OR. `/` is cargo's old spelling of OR."""
    tokens = re.findall(r"[()]|[^\s()/]+|/", expression)
    tokens = ["OR" if token == "/" else token for token in tokens]
    position = 0

    def peek():
        return tokens[position] if position < len(tokens) else None

    def take():
        nonlocal position
        position += 1
        return tokens[position - 1]

    def either():
        value = both()
        while peek() == "OR":
            take()
            value = both() or value
        return value

    def both():
        value = single()
        while peek() == "AND":
            take()
            value = single() and value
        return value

    def single():
        token = peek()
        if token is None or token in (")", "AND", "OR", "WITH"):
            raise ValueError(expression)
        take()
        if token == "(":
            value = either()
            if peek() != ")":
                raise ValueError(expression)
            take()
            return value
        if peek() == "WITH":
            take()
            if peek() is None:
                raise ValueError(expression)
            return f"{token} WITH {take()}" in allowed
        return token in allowed

    try:
        value = either()
    except ValueError:
        return False
    return value and position == len(tokens)
