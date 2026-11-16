#!/usr/bin/env python3
"""Benchmark regression gate: build the benchmarks at two commits, run both, compare.

The Performance workflow (.github/workflows/perf.yml) runs this on every pull
request that touches the engine or the core. It works on any machine with the
engine's build requirements, so a regression the workflow reports can be
reproduced locally:

    python3 tools/perf/bench_gate.py all --base origin/main            # Linux: instructions + time
    python3 tools/perf/bench_gate.py all --base origin/main --mode time  # macOS / Windows

Two measurements:

  instructions  Each benchmark runs once per side under Valgrind's Callgrind
                (Linux only). Callgrind starts a new count every time the
                benchmark prints a line, so every table row gets the CPU
                instructions spent producing it, all threads included. The
                counts repeat almost exactly from run to run, so a row more
                than --limit percent (default 1) above the base fails the gate.
  time          Each benchmark runs natively, base and head back to back, for
                --rounds rounds; the fastest round of each row counts (noise
                only ever adds time). Shared CI machines are noisy, so a row
                more than --time-limit percent (default 15) slower is re-timed
                with more rounds and reported as a warning, never a failure.

The benchmarks print human-readable tables; this script reads them as they
are, so a new benchmark row is picked up without changes here. A row is a line
that ends in numbers; the line(s) above a table name it. Columns whose header
or value has a time unit (us, ms, avg, p99, median ...) are measurements, the
rest (tracks, notes ...) are part of the row's name.

Subcommands:
  build    build the benchmarks of one source tree into a directory
  run      run built benchmarks (one or more sides) and save the results
  compare  compare two result files, write a Markdown report, set the exit code
  all      build base (a git worktree) and head (this checkout), run, compare

Exit codes: 0 = no regression (or --accept), 1 = regression, 2 = setup error or a benchmark
that could not be measured with the change (--accept does not cover that).
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""

# Benchmarks and their arguments. The instruction runs go through Valgrind
# (~50x slower than native), so they use short runs: instruction counts do
# not need many blocks to be stable. The time runs use enough blocks for a
# usable average while keeping three interleaved rounds of base and head
# within a few minutes on a hosted runner.
ENGINE_BENCHES = {
    # name: (instruction args, time args)
    "dstudio_engine_bench": (["20"], ["400"]),
    "dstudio_device_bench": (["200"], ["2000"]),
    "dstudio_warp_bench": (["20"], ["400"]),
    "dstudio_clip_loop_bench": (["100"], ["1000"]),
    "dstudio_decode_bench": (["1"], ["5"]),
    "dstudio_offline_plugin_bench": (["1"], ["5"]),
    "dstudio_scan_cache_bench": (["500", "3"], ["2000", "20"]),
    "dstudio_pcm_convert_bench": (["200"], ["20000"]),
}
RUST_BENCH_PACKAGE = "dstudio-core"  # its [[bench]] targets take no arguments

# Callgrind dumps its counters before each of these calls, so every printed
# line gets its own count. C/C++: stdio (glibc's fortified printf is
# __printf_chk). Rust: println! goes through std::io::stdio::_print.
PRINT_FUNCTIONS = ["printf", "__printf_chk", "puts", "putchar", "fputs", "std::io::stdio::_print"]

# Rows under this many instructions are listed but never fail the gate: one
# extra allocation is a large percentage of nothing.
MIN_GATED_INSTRUCTIONS = 1_000_000


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def run(cmd: list[str], cwd: Path | None = None, log_file: Path | None = None, env: dict | None = None) -> None:
    log("$ " + " ".join(cmd))
    if log_file is None:
        subprocess.run(cmd, cwd=cwd, check=True, env=env)
        return
    with open(log_file, "w") as f:
        r = subprocess.run(cmd, cwd=cwd, stdout=f, stderr=subprocess.STDOUT, env=env)
    if r.returncode != 0:
        log(Path(log_file).read_text(errors="replace")[-4000:])
        raise subprocess.CalledProcessError(r.returncode, cmd)


# --------------------------------------------------------------------- build

def build(src: Path, out: Path, deps: Path | None) -> Path:
    """Builds the engine benchmarks and the core's Rust benchmarks of `src`
    into `out`; returns the manifest (benchmark name -> executable)."""
    out.mkdir(parents=True, exist_ok=True)
    engine_build = out / "engine"
    cfg = ["cmake", "-S", str(src / "engine"), "-B", str(engine_build), "-G", "Ninja",
           "-DCMAKE_BUILD_TYPE=Release", "-DDSTUDIO_ENGINE_BUILD_TESTS=OFF", "-DDSTUDIO_ENGINE_BUILD_TOOLS=OFF"]
    if deps is not None:
        cfg.append(f"-DFETCHCONTENT_BASE_DIR={deps}")
    if shutil.which("ccache"):
        cfg += ["-DCMAKE_C_COMPILER_LAUNCHER=ccache", "-DCMAKE_CXX_COMPILER_LAUNCHER=ccache"]
    run(cfg, log_file=out / "engine-configure.log")
    targets = [t for t in ENGINE_BENCHES if (src / "engine" / "CMakeLists.txt").read_text().count(f"add_executable({t} ")]
    run(["cmake", "--build", str(engine_build), "--target", *targets], log_file=out / "engine-build.log")
    manifest: dict[str, dict] = {}
    for t in targets:
        exe = engine_build / (t + EXE)
        if exe.exists():
            manifest[t] = {"exe": str(exe), "kind": "engine"}

    env = dict(os.environ, CARGO_TARGET_DIR=str(out / "cargo"))
    cargo = ["cargo", "bench", "-p", RUST_BENCH_PACKAGE, "--no-run", "--locked", "--message-format=json"]
    log("$ " + " ".join(cargo))
    with open(out / "cargo-build.log", "w") as err:
        r = subprocess.run(cargo, cwd=src, env=env, stdout=subprocess.PIPE, stderr=err, text=True)
    if r.returncode != 0:
        # With --message-format=json the compiler's errors are on stdout.
        for line in r.stdout.splitlines():
            try:
                m = json.loads(line)
            except json.JSONDecodeError:
                continue
            if m.get("reason") == "compiler-message" and m["message"].get("level") == "error":
                log(m["message"].get("rendered") or m["message"].get("message", ""))
        log((out / "cargo-build.log").read_text(errors="replace")[-4000:])
        raise subprocess.CalledProcessError(r.returncode, cargo)
    for line in r.stdout.splitlines():
        try:
            m = json.loads(line)
        except json.JSONDecodeError:
            continue
        if m.get("reason") == "compiler-artifact" and m.get("executable") and "bench" in m["target"]["kind"]:
            manifest[f"core/{m['target']['name']}"] = {"exe": m["executable"], "kind": "rust"}

    path = out / "manifest.json"
    path.write_text(json.dumps(manifest, indent=2))
    log(f"built {len(manifest)} benchmarks into {out}")
    return path


# --------------------------------------------------------------------- parse

NUM = re.compile(r"^[-+]?(\d+(\.\d*)?|\.\d+)(e[-+]?\d+)?\s*(x|%|us|µs|ms|ns|s)?$", re.I)
UNIT_VALUE = re.compile(r"(us|µs|ms|ns|s)$", re.I)
MEASURE_HEADER = re.compile(
    r"\bus\b|µs|\bms\b|\bns\b|/s\b|\bp\d\d\b|\bmax\b|\bmin\b|\bavg\b|\bmedian\b|\bmean\b|real[- ]?time|"
    r"\bratio\b|per core|per track|per clip|per entry|\bthroughput\b|\bMB\b|\bGB\b|%|\bworker\b|\bprepare\b|\bdestroy\b",
    re.I)
HIGHER_IS_BETTER = re.compile(r"real[- ]?time|\bratio\b|per core|\bthroughput\b|/s\b|MB/s|GB/s", re.I)
PRIMARY = re.compile(r"\bavg\b|\bmedian\b|\bmean\b|us/block", re.I)


def split_cells(line: str) -> list[str]:
    s = line.strip()
    if s.startswith("|"):
        return [c.strip() for c in s.strip("|").split("|")]
    return [c for c in re.split(r"\s{2,}", s) if c]


def is_separator(line: str) -> bool:
    return bool(re.fullmatch(r"\|?(\s*:?-{2,}:?\s*\|)+\s*:?-*:?\s*\|?", line.strip()))


def parse_number(cell: str) -> float | None:
    if not NUM.match(cell):
        return None
    return float(re.sub(r"\s*(x|%|us|µs|ms|ns|s)$", "", cell, flags=re.I))


def parse_table_text(text: str) -> list[dict]:
    """Reads benchmark output. Returns one record per non-empty line, in order:
    {"line", "key", "row": bool, "section", "label", "values": {column: (value, higher_is_better)}, "primary"}.
    Every line gets a key (table lines too, with their numbers masked), so the
    instruction counts of setup work and of one-line results are compared as
    well; only rows carry values for the time comparison."""
    records: list[dict] = []
    context: list[str] = []      # non-row lines since the last row
    section = ""
    header: list[str] = []
    seen: dict[str, int] = {}
    last_was_row = False

    def unique(key: str) -> str:
        seen[key] = seen.get(key, 0) + 1
        return key if seen[key] == 1 else f"{key} #{seen[key]}"

    for raw in text.splitlines():
        line = raw.rstrip()
        if not line.strip():
            continue
        masked = re.sub(r"\d+(\.\d+)?", "#", line.strip())
        masked = re.sub(r"\s+", " ", masked)
        rec = {"line": line, "row": False, "section": "", "label": masked, "key": unique(f"line: {masked}")}
        records.append(rec)
        if is_separator(line):
            rec["separator"] = True
            continue
        cells = split_cells(line)
        nums = [parse_number(c) for c in cells]
        if not any(n is not None for n in nums) or len(cells) < 2:
            if last_was_row:
                context = []
            context.append(line.strip())
            if len(cells) >= 2:
                header = cells
            last_was_row = False
            continue
        # A row. Its section is the table's title: the line above the column
        # header, or the header itself when the table has no title.
        if not last_was_row:
            section = context[-2] if len(context) >= 2 else (context[-1] if context else "")
            section = re.sub(r"\s{2,}", "  ", section)
        last_was_row = True
        if len(cells) >= len(header):
            hdr = header + [""] * (len(cells) - len(header))
        else:
            hdr = header[-len(cells):]
        label_parts: list[str] = []
        values: dict[str, tuple[float, bool]] = {}
        for i, (cell, num, name) in enumerate(zip(cells, nums, hdr)):
            measure = num is not None and (bool(MEASURE_HEADER.search(name)) or bool(UNIT_VALUE.search(cell))
                                           or (not name and i == len(cells) - 1 and cell.endswith("x")))
            if measure:
                values[name or f"col{i}"] = (num, bool(HIGHER_IS_BETTER.search(name)) or cell.endswith("x"))
            else:
                label_parts.append(cell)
        if not values:
            # No header told us which columns are measurements: the trailing
            # numbers are, the leading cells name the row.
            k = len(cells)
            while k > 1 and nums[k - 1] is not None:
                k -= 1
            label_parts = cells[:k]
            for i in range(k, len(cells)):
                values[hdr[i] or f"col{i}"] = (nums[i], cells[i].endswith("x"))
        label = " | ".join(label_parts)
        seen[rec["key"]] -= 1
        primary = next((c for c in values if PRIMARY.search(c) and not values[c][1]), None)
        if primary is None:
            primary = next((c for c in values if not values[c][1]), None)
        title = re.sub(r"\s+", " ", re.sub(r"\d+(\.\d+)?", "#", section))
        rec.update(row=True, key=unique(f"{title} » {label}"), section=section, label=label, values=values,
                   primary=primary)
    return records


# ----------------------------------------------------------------------- run

def bench_args(name: str, mode: str) -> list[str]:
    if name in ENGINE_BENCHES:
        return ENGINE_BENCHES[name][0 if mode == "instructions" else 1]
    return []


def private_temp(directory: str) -> dict:
    """Environment whose temp directory is `directory`: the benchmarks write
    their test files under fixed names in the temp directory, so two runs at
    once (base and head) must not share it."""
    return dict(os.environ, TMPDIR=directory, TMP=directory, TEMP=directory)


def run_instructions(name: str, exe: str, work: Path) -> dict:
    """One Callgrind run; returns {"rows": {key: instructions}, "total": n, "note": str}."""
    cg_dir = work / "cg" / name.replace("/", "_")
    shutil.rmtree(cg_dir, ignore_errors=True)
    cg_dir.mkdir(parents=True)
    out_file = cg_dir / "callgrind.out"
    cmd = ["valgrind", "--tool=callgrind", "--cache-sim=no", "--branch-sim=no",
           f"--callgrind-out-file={out_file}"] + [f"--dump-before={f}" for f in PRINT_FUNCTIONS]
    cmd += [exe, *bench_args(name, "instructions")]
    log("$ " + " ".join(cmd))
    with tempfile.TemporaryDirectory() as cwd:
        r = subprocess.run(cmd, cwd=cwd, env=private_temp(cwd), capture_output=True, text=True, errors="replace")
    (cg_dir / "stdout.txt").write_text(r.stdout)
    (cg_dir / "stderr.txt").write_text(r.stderr)
    if r.returncode != 0:
        return {"error": f"exit status {r.returncode}: {r.stderr.strip().splitlines()[-1:] }"}
    dumps: list[tuple[int, int]] = []  # (part number, instructions); part 0 = program end
    for f in cg_dir.glob("callgrind.out*"):
        m = re.fullmatch(r"callgrind\.out(?:\.(\d+))?", f.name)
        if not m:
            continue
        tot = 0
        for line in f.read_text(errors="replace").splitlines():
            if line.startswith("totals:") or line.startswith("summary:"):
                tot = int(line.split()[1])
                if line.startswith("totals:"):
                    break
        dumps.append((int(m.group(1)) if m.group(1) else 10**9, tot))
    dumps.sort()
    counts = [n for _, n in dumps]
    total = sum(counts)
    if total == 0:
        return {"error": "Callgrind wrote no instruction counts"}
    all_records = parse_table_text(r.stdout)
    # One dump before every print call, plus one at exit. The benchmarks print
    # one non-empty line per call (a Markdown table's header and separator
    # sometimes in one call), so dump i is the work done before line i.
    printed = counts[:-1]
    result: dict = {"total": total, "rows": {}, "labels": {}}
    # A call that prints only a newline (Rust's `println!()`) has no line of
    # its own: its dump holds only the formatting since the previous line, so
    # it is folded into the next one.
    def fit(n_lines: int) -> list[int] | None:
        dumps = list(printed)
        while len(dumps) > n_lines:
            i = min(range(len(dumps)), key=dumps.__getitem__)
            if dumps[i] > 100_000:
                return None
            if i + 1 < len(dumps):
                dumps[i + 1] += dumps[i]
            del dumps[i]
        return dumps if len(dumps) == n_lines else None

    records, fitted = None, None
    for cand in ([rec for rec in all_records if not rec.get("separator")], all_records):
        fitted = fit(len(cand))
        if fitted is not None:
            records = cand
            break
    if records is None:
        result["note"] = (f"{len(printed)} print calls for {len(all_records)} lines: per-row counts unavailable, "
                          "only the total is compared")
        return result
    for rec, n in zip(records, fitted):
        result["rows"][rec["key"]] = n
        result["labels"][rec["key"]] = [rec["section"], rec["label"]]
    return result


def run_native(name: str, exe: str) -> dict:
    with tempfile.TemporaryDirectory() as cwd:
        r = subprocess.run([exe, *bench_args(name, "time")], cwd=cwd, env=private_temp(cwd), capture_output=True,
                           text=True, errors="replace")
    if r.returncode != 0:
        return {"error": f"exit status {r.returncode}", "stdout": r.stdout, "stderr": r.stderr[-2000:]}
    rows = {}
    for rec in parse_table_text(r.stdout):
        if rec["row"] and rec["primary"]:
            rows[rec["key"]] = {"value": rec["values"][rec["primary"]][0], "column": rec["primary"],
                                "section": rec["section"], "label": rec["label"]}
    return {"rows": rows, "stdout": r.stdout}


def merge_fastest(acc: dict, res: dict) -> None:
    if "error" in res:
        acc.setdefault("error", res["error"])
        return
    for k, row in res["rows"].items():
        cur = acc.setdefault("rows", {}).get(k)
        if cur is None:
            acc["rows"][k] = dict(row, rounds=[row["value"]])
        else:
            cur["rounds"].append(row["value"])
            cur["value"] = min(cur["value"], row["value"])
    acc["stdout"] = res.get("stdout", "")


def run_sides(sides: dict[str, dict], mode: str, rounds: int, work: Path, only: set[str] | None) -> dict[str, dict]:
    """Runs every benchmark on every side. Instruction counts do not depend on
    what else runs, so those runs go in parallel (Valgrind runs a program's
    threads one at a time, so each run uses one core). Time mode runs one
    benchmark at a time and interleaves the sides (base, head, base, head ...)
    so slow and fast moments of a shared machine land on both."""
    names = sorted(set().union(*[set(m) for m in sides.values()]))
    if only:
        names = [n for n in names if n in only]
    results: dict[str, dict] = {s: {} for s in sides}
    if mode == "instructions":
        from concurrent.futures import ThreadPoolExecutor

        def one(side: str, name: str) -> None:
            t = time.time()
            results[side][name] = run_instructions(name, sides[side][name]["exe"], work / side)
            log(f"  [{side}] {name}: {time.time() - t:.0f} s")

        jobs = [(side, name) for name in names for side in sides if name in sides[side]]
        with ThreadPoolExecutor(max_workers=max(1, os.cpu_count() or 1)) as pool:
            for f in [pool.submit(one, side, name) for side, name in jobs]:
                f.result()
        return results
    for name in names:
        for rnd in range(rounds):
            for side, manifest in sides.items():
                if name in manifest:
                    t = time.time()
                    merge_fastest(results[side].setdefault(name, {}), run_native(name, manifest[name]["exe"]))
                    log(f"  [{side}] {name} round {rnd + 1}/{rounds}: {time.time() - t:.1f} s")
    return results


# ------------------------------------------------------------------- compare

def pct(base: float, head: float) -> float:
    return (head - base) / base * 100.0 if base else 0.0


def compare_instructions(base: dict, head: dict, limit: float) -> tuple[list[dict], list[str], list[str]]:
    """Returns (rows, notes, errors). An error is a head benchmark that could not be measured:
    it fails the check whatever the accept label says, so a broken runner never passes."""
    rows, notes, errors = [], [], []
    for name in sorted(set(base) | set(head)):
        b, h = base.get(name), head.get(name)
        if h is not None and "error" in h:
            errors.append(f"`{name}` could not be measured: {h['error']}")
            continue
        if b is None or h is None:
            notes.append(f"`{name}` is {'new' if b is None else 'gone'} in this change")
            continue
        if "error" in b:
            # Nothing to compare against. This fails like a regression, so the gate never
            # passes unmeasured; a change that repairs the benchmark takes the accept label.
            notes.append(f"`{name}` failed at the base: {b['error']}")
            rows.append({"bench": name, "key": "(run)", "section": "", "label": "base run failed, nothing to compare",
                         "base": 0, "head": h["total"], "pct": float("inf"), "fail": True})
            continue
        for side, r in (("base", b), ("head", h)):
            if r.get("note"):
                notes.append(f"`{name}` ({side}): {r['note']}")
        keys = sorted(set(b["rows"]) & set(h["rows"]), key=lambda k: list(h["rows"]).index(k))
        new = set(h["rows"]) - set(b["rows"])
        if new:
            notes.append(f"`{name}`: {len(new)} new row(s) with no base to compare")
        if not keys:
            keys = []
            rows.append({"bench": name, "key": "(total)", "section": "", "label": "whole run",
                         "base": b["total"], "head": h["total"], "pct": pct(b["total"], h["total"]),
                         "fail": pct(b["total"], h["total"]) > limit})
        for k in keys:
            bv, hv = b["rows"][k], h["rows"][k]
            p = pct(bv, hv)
            section, label = h["labels"][k]
            rows.append({"bench": name, "key": k, "section": section, "label": label, "base": bv, "head": hv,
                         "pct": p, "fail": p > limit and max(bv, hv) >= MIN_GATED_INSTRUCTIONS})
    return rows, notes, errors


def compare_time(base: dict, head: dict, limit: float) -> tuple[list[dict], list[str]]:
    rows, notes = [], []
    for name in sorted(set(base) | set(head)):
        b, h = base.get(name), head.get(name)
        if b is None or h is None:
            notes.append(f"`{name}` is {'new' if b is None else 'gone'} in this change")
            if h is not None and "error" in h:
                notes.append(f"`{name}` failed: base absent, head {h['error']}")
            continue
        if "error" in b or "error" in h:
            notes.append(f"`{name}` failed: base {b.get('error', 'ok')}, head {h.get('error', 'ok')}")
            continue
        if "retime_error" in b or "retime_error" in h:
            notes.append(f"`{name}` failed: base {b.get('retime_error', 'ok')}, head {h.get('retime_error', 'ok')}"
                         " (while re-timing; first-round rows kept)")
        for k, hr in h["rows"].items():
            br = b["rows"].get(k)
            if br is None:
                continue
            p = pct(br["value"], hr["value"])
            rows.append({"bench": name, "key": k, "section": hr["section"], "label": hr["label"],
                         "column": hr["column"], "base": br["value"], "head": hr["value"], "pct": p,
                         "fail": p > limit, "rounds": len(hr["rounds"])})
    return rows, notes


def fmt_count(n: float) -> str:
    for unit, div in (("G", 1e9), ("M", 1e6), ("k", 1e3)):
        if abs(n) >= div:
            return f"{n / div:.2f} {unit}"
    return f"{n:.0f}"


def markdown(kind: str, rows: list[dict], notes: list[str], limit: float, title: str, labels: tuple[str, str],
             accept: bool, errors: list[str] | None = None) -> str:
    fails = [r for r in rows if r["fail"]]
    gains = [r for r in rows if r["pct"] < -limit and (kind != "instructions" or max(r["base"], r["head"]) >= MIN_GATED_INSTRUCTIONS)]
    out = [f"### {title}", ""]
    out.append(f"Base `{labels[0]}`, head `{labels[1]}`. {len(rows)} rows compared; limit +{limit:g} %.")
    out.append("")
    if fails:
        if kind == "time":
            verdict = "slower, a warning only (timings on shared runners do not fail the check)"
        elif accept:
            verdict = "accepted (label `perf-regression-accepted`)"
        else:
            verdict = "**regression**"
        out.append(f"{len(fails)} row(s) over the limit: {verdict}.")
    else:
        out.append("No row over the limit.")
    if gains:
        out.append(f"{len(gains)} row(s) improved by more than {limit:g} %.")
    if errors:
        out.append("")
        out.append(f"**{len(errors)} benchmark(s) could not be measured; the check fails (the accept label does not cover this):**")
        out.append("")
        out += [f"- {e}" for e in errors]
    out.append("")

    def cell(text: str, limit: int = 70) -> str:
        text = text.strip().strip("|").strip()
        return (text[:limit] + "…" if len(text) > limit else text).replace("|", "\\|")

    def table(sel: list[dict], heading: str) -> None:
        if not sel:
            return
        out.extend([f"**{heading}**", ""])
        if kind == "instructions":
            out.append("| benchmark | table | row | base | head | change |")
            out.append("|---|---|---|---:|---:|---:|")
            for r in sel:
                out.append(f"| {r['bench']} | {cell(r['section'])} | {cell(r['label'])} | {fmt_count(r['base'])} | "
                           f"{fmt_count(r['head'])} | {r['pct']:+.2f} % |")
        else:
            out.append("| benchmark | table | row | column | base | head | change | rounds |")
            out.append("|---|---|---|---|---:|---:|---:|---:|")
            for r in sel:
                out.append(f"| {r['bench']} | {cell(r['section'])} | {cell(r['label'])} | {cell(r['column'])} | {r['base']:g} | "
                           f"{r['head']:g} | {r['pct']:+.1f} % | {r['rounds']} |")
        out.append("")

    table(sorted(fails, key=lambda r: -r["pct"]), "Over the limit")
    table(sorted(gains, key=lambda r: r["pct"]), "Improved")
    if notes:
        out.extend(["**Notes**", ""] + [f"- {n}" for n in notes] + [""])
    out.append("<details><summary>All rows</summary>")
    out.append("")
    table(rows, "All rows")
    out.append("</details>")
    out.append("")
    return "\n".join(out)


# ------------------------------------------------------------------ commands

def load_manifest(p: str) -> dict:
    return json.loads(Path(p).read_text())


def cmd_build(a) -> int:
    build(Path(a.src).resolve(), Path(a.out).resolve(), Path(a.deps).resolve() if a.deps else None)
    return 0


def cmd_run(a) -> int:
    sides = {}
    for s in a.side:
        name, _, path = s.partition("=")
        sides[name] = load_manifest(path)
    res = run_sides(sides, a.mode, a.rounds, Path(a.work).resolve(), set(a.only) if a.only else None)
    for name, r in res.items():
        Path(a.out_dir).mkdir(parents=True, exist_ok=True)
        (Path(a.out_dir) / f"{a.mode}-{name}.json").write_text(json.dumps(r, indent=2))
    return 0


def report(mode: str, base: dict, head: dict, a, labels: tuple[str, str]) -> tuple[str, bool, bool]:
    """Returns (markdown, a row over the limit, a head benchmark that could not be measured)."""
    errors: list[str] = []
    if mode == "instructions":
        rows, notes, errors = compare_instructions(base, head, a.limit)
        md = markdown("instructions", rows, notes, a.limit, "CPU instructions per benchmark row (Callgrind)",
                      labels, a.accept, errors)
    else:
        rows, notes = compare_time(base, head, a.time_limit)
        md = markdown("time", rows, notes, a.time_limit,
                      f"Wall time per benchmark row, fastest of each side ({platform.system()})", labels, a.accept)
    return md, any(r["fail"] for r in rows), bool(errors)


def cmd_compare(a) -> int:
    base = json.loads(Path(a.base).read_text())
    head = json.loads(Path(a.head).read_text())
    md, failed, broken = report(a.mode, base, head, a, (a.base_label, a.head_label))
    print(md)
    if a.markdown:
        Path(a.markdown).write_text(md)
    if broken:
        return 2
    return 1 if failed and a.mode == "instructions" and not a.accept else 0


def git(*args: str, cwd: Path = ROOT) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def cmd_all(a) -> int:
    work = Path(a.work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    base_src = work / "src-base"
    if base_src.exists():
        subprocess.run(["git", "worktree", "remove", "--force", str(base_src)], cwd=ROOT, capture_output=True)
        shutil.rmtree(base_src, ignore_errors=True)
    run(["git", "worktree", "add", "--detach", str(base_src), a.base], cwd=ROOT)
    labels = (git("rev-parse", "--short", "HEAD", cwd=base_src), git("rev-parse", "--short", "HEAD"))
    deps = Path(a.deps).resolve() if a.deps else None
    try:
        mb = build(base_src, work / "base", deps / "base" if deps else None)
    except subprocess.CalledProcessError:
        log("bench_gate: the BASE benchmarks did not build; nothing to compare against")
        return 2
    mh = build(ROOT, work / "head", deps / "head" if deps else None)
    sides = {"base": load_manifest(str(mb)), "head": load_manifest(str(mh))}
    # The benchmark list comes from head: a benchmark removed by the change is
    # not run, one added by it runs on head only.
    modes = ["instructions", "time"] if a.mode == "both" else [a.mode]
    if "instructions" in modes and not shutil.which("valgrind"):
        log("bench_gate: valgrind not found; instruction counts need Linux + valgrind")
        return 2
    failed = broken = False
    reports = []
    for mode in modes:
        res = run_sides(sides, mode, a.rounds, work, set(a.only) if a.only else None)
        if mode == "time":
            # Re-time the rows over the limit with more rounds; the fastest of
            # all rounds decides.
            rows, *_ = compare_time(res["base"], res["head"], a.time_limit)
            suspects = sorted({r["bench"] for r in rows if r["fail"]})
            if suspects and a.confirm_rounds:
                log(f"re-timing {len(suspects)} benchmark(s) over the time limit: {', '.join(suspects)}")
                more = run_sides(sides, mode, a.confirm_rounds, work, set(suspects))
                for side in res:
                    for name, r in more[side].items():
                        acc = res[side].setdefault(name, {})
                        if "error" in r:  # a crash while re-timing: keep the first rows, note the crash
                            acc.setdefault("retime_error", r["error"])
                        for k, row in r.get("rows", {}).items():
                            if k in acc.get("rows", {}):
                                acc["rows"][k]["rounds"] += row["rounds"]
                                acc["rows"][k]["value"] = min(acc["rows"][k]["value"], row["value"])
        for side, r in res.items():
            (work / f"{mode}-{side}.json").write_text(json.dumps(r, indent=2))
        md, f, b = report(mode, res["base"], res["head"], a, labels)
        reports.append(md)
        failed |= f and mode == "instructions"
        broken |= b
    text = "\n".join(reports)
    (work / "report.md").write_text(text)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(text + "\n")
    if broken:
        return 2  # the accept label never covers a benchmark that could not be measured
    return 1 if failed and not a.accept else 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    b = sub.add_parser("build", help="build one tree's benchmarks")
    b.add_argument("--src", default=str(ROOT))
    b.add_argument("--out", required=True)
    b.add_argument("--deps", help="FetchContent download dir (shared between runs to skip downloads)")
    b.set_defaults(fn=cmd_build)

    r = sub.add_parser("run", help="run built benchmarks")
    r.add_argument("--side", action="append", required=True, metavar="NAME=MANIFEST")
    r.add_argument("--mode", choices=["instructions", "time"], required=True)
    r.add_argument("--rounds", type=int, default=3)
    r.add_argument("--only", nargs="*")
    r.add_argument("--work", default="build/perf")
    r.add_argument("--out-dir", default="build/perf")
    r.set_defaults(fn=cmd_run)

    c = sub.add_parser("compare", help="compare two result files")
    c.add_argument("--mode", choices=["instructions", "time"], required=True)
    c.add_argument("--base", required=True)
    c.add_argument("--head", required=True)
    c.add_argument("--base-label", default="base")
    c.add_argument("--head-label", default="head")
    c.add_argument("--markdown")
    c.set_defaults(fn=cmd_compare)

    al = sub.add_parser("all", help="build base and head, run, compare")
    al.add_argument("--base", required=True, help="git ref to compare against")
    al.add_argument("--mode", choices=["instructions", "time", "both"], default="both")
    al.add_argument("--rounds", type=int, default=3)
    al.add_argument("--confirm-rounds", type=int, default=4)
    al.add_argument("--only", nargs="*", help="benchmark names to run (default: all)")
    al.add_argument("--work", default=str(ROOT / "build" / "perf"))
    al.add_argument("--deps", help="FetchContent download dirs (base/ and head/ under it)")
    al.set_defaults(fn=cmd_all)

    for sp in (c, al):
        sp.add_argument("--limit", type=float, default=float(os.environ.get("PERF_LIMIT_PCT", "1")),
                        help="allowed growth in instructions per row, percent (default 1)")
        sp.add_argument("--time-limit", type=float, default=float(os.environ.get("PERF_TIME_LIMIT_PCT", "15")),
                        help="time growth per row that is reported as a warning, percent (default 15)")
        sp.add_argument("--accept", action="store_true", default=bool(os.environ.get("PERF_ACCEPT")),
                        help="report regressions but exit 0")
    a = p.parse_args()
    try:
        return a.fn(a)
    except subprocess.CalledProcessError as e:
        log(f"bench_gate: command failed: {e}")
        return 2


if __name__ == "__main__":
    sys.exit(main())
