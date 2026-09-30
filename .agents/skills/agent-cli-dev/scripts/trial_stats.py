#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Summarize development trials from Claude Code stream-json logs.

For each log: tokens in (all, and uncached) and out, tool calls, turns, wall
time, cost, and the files the agent read (Read calls, and cat/sed/head/tail
in Bash commands).
"""

import argparse
import json
import re
import sys
from pathlib import Path

SHELL_READ = re.compile(
    r"(?:cat|sed -n \S+|head(?: -n)?(?: -?\d+)?|tail(?: -n)?(?: -?\d+)?)\s+([\w./-]+\.(?:rs|md|toml|json|sh))"
)


def summarize(path: Path) -> str:
    tools, reads, result = 0, [], {}
    for line in path.read_text().splitlines():
        event = json.loads(line)
        if event.get("type") == "assistant":
            for part in event["message"].get("content", []):
                if part.get("type") != "tool_use":
                    continue
                tools += 1
                given = part["input"]
                if part["name"] == "Read":
                    reads.append(given["file_path"])
                elif part["name"] == "Bash":
                    reads += SHELL_READ.findall(given["command"])
        elif event.get("type") == "result":
            result = event
    if not result:
        sys.exit(f"{path}: no result event; the run did not finish")
    usage = result.get("usage", {})
    uncached = usage.get("input_tokens", 0) + usage.get("cache_creation_input_tokens", 0)
    total = uncached + usage.get("cache_read_input_tokens", 0)
    return "\n".join(
        [
            f"== {path.name}",
            f"tokens in {total:,} (uncached {uncached:,}), out {usage.get('output_tokens', 0):,}; "
            f"tool calls {tools}; turns {result.get('num_turns')}; "
            f"wall {result.get('duration_ms', 0) / 1000:.0f}s; cost ${result.get('total_cost_usd', 0):.2f}",
            "read: " + ", ".join(dict.fromkeys(reads)),
        ]
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", nargs="+", type=Path, help="stream-json logs from `claude -p`")
    args = parser.parse_args()
    for log in args.logs:
        print(summarize(log))


if __name__ == "__main__":
    main()
