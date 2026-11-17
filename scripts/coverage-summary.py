"""Summarize LLVM line coverage excluding inline and separate test modules."""
from pathlib import Path
import sys
from coverage_source import excluded_test_lines

source = Path(sys.argv[1])
covered = total = 0
rows = []
for record in source.read_text(encoding="utf-8-sig").split("end_of_record"):
    lines = record.strip().splitlines()
    filename = next((line[3:] for line in lines if line.startswith("SF:")), None)
    if not filename or filename.endswith(("_tests.rs", "_test_helpers.rs")):
        continue
    path = Path(filename)
    contents = path.read_text(encoding="utf-8").splitlines()
    excluded = excluded_test_lines("\n".join(contents), extended="--extended" in sys.argv[2:])
    hits = [tuple(map(int, line[3:].split(",")[:2]))
            for line in lines if line.startswith("DA:")]
    production = [(line, count) for line, count in hits if line not in excluded]
    count = len(production)
    passed = sum(count > 0 for _, count in production)
    covered += passed
    total += count
    rows.append(f"{path.name}: {passed}/{count} executable production lines")
percent = covered / total * 100 if total else 0
summary = "\n".join(rows + [f"Production line coverage: {covered}/{total} = {percent:.2f}%"])
print(summary)
source.with_name("production-summary.txt").write_text(summary + "\n", encoding="utf-8")
if percent < 80:
    raise SystemExit("Production line coverage is below 80%")
