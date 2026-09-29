#!/usr/bin/env python3
"""Plot Figure 13, or another artifact figure when given its number."""
import sys
from pathlib import Path
from plot_figures import main

if __name__ == "__main__":
    if len(sys.argv) == 1:
        sys.argv.extend(["13", str(Path(__file__).resolve().parents[1] / "result" / "fig13")])
    elif len(sys.argv) == 2:
        sys.argv.insert(1, "13")
    raise SystemExit(main())
