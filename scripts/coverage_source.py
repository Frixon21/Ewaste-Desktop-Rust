"""Locate test-only Rust items without excluding later production items."""
import bisect
import re


def _code_mask(source):
    """Blank strings/comments while preserving offsets and line numbers."""
    masked = list(source)
    pos = 0
    while pos < len(source):
        end = pos
        if source.startswith("//", pos):
            end = source.find("\n", pos)
            if end < 0:
                end = len(source)
        elif source.startswith("/*", pos):
            end, depth = pos + 2, 1
            while end < len(source) and depth:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
        else:
            raw = re.match(r'(?:br|r)(\#*)"', source[pos:])
            char = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", source[pos:])
            if raw:
                marker = '"' + raw.group(1)
                close = source.find(marker, pos + raw.end())
                end = len(source) if close < 0 else close + len(marker)
            elif source[pos] == '"':
                end = pos + 1
                while end < len(source):
                    if source[end] == "\\":
                        end += 2
                    elif source[end] == '"':
                        end += 1
                        break
                    else:
                        end += 1
            elif char:
                end = pos + char.end()
        if end > pos:
            for index in range(pos, end):
                if masked[index] != "\n":
                    masked[index] = " "
            pos = end
        else:
            pos += 1
    return "".join(masked)


def _item_end(code, start):
    while start < len(code):
        if code[start].isspace():
            start += 1
        elif code.startswith("#[", start):
            close = code.find("]", start)
            if close < 0:
                raise ValueError("Unclosed Rust attribute in coverage source")
            start = close + 1
        else:
            break
    is_item = re.match(r"(?:(?:pub(?:\([^)]*\))?|async|unsafe|default|extern)\s+)*(?:fn|impl|mod|struct|enum|trait|type|use|const|static)\b", code[start:])
    braces = parens = brackets = 0
    opened = False
    for pos in range(start, len(code)):
        char = code[pos]
        if char == "(":
            parens += 1
        elif char == ")":
            parens -= 1
        elif char == "[":
            brackets += 1
        elif char == "]":
            brackets -= 1
        elif char == "{":
            braces += 1
            opened = True
        elif char == "}":
            braces -= 1
            if opened and braces == 0:
                return pos
        elif not (braces or parens or brackets) and (char == ";" or char == "," and not is_item):
            return pos
    raise ValueError("Could not locate complete test-only Rust item")


def excluded_test_lines(source, extended=False):
    code = _code_mask(source)
    starts = [0] + [match.end() for match in re.finditer("\n", source)]
    excluded = set()
    for match in re.finditer(r"#\[cfg\(([^\n]+)\)\]", source):
        if code[match.start():match.start() + 2] != "#[":
            continue
        expression = re.sub(r"\s+", "", match.group(1))
        test_only = expression == "test" or expression.startswith("all(test,")
        test_only |= expression == 'any(test,feature="extended-workflows")' and not extended
        test_only |= expression == 'any(test,not(feature="extended-workflows"))' and extended
        if not test_only:
            continue
        end = _item_end(code, match.end())
        excluded.update(range(bisect.bisect_right(starts, match.start()), bisect.bisect_right(starts, end) + 1))
    return excluded
