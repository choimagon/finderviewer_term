"""Command-line entry point."""
import argparse
from pathlib import Path
import sys

from . import __version__
from .app import FinderViewApp


def main() -> None:
    parser=argparse.ArgumentParser(prog="finderviewer",description="Finder-style column explorer for your terminal")
    parser.add_argument("path",nargs="?",default=".",help="Starting folder (default: current directory)")
    parser.add_argument("--version",action="version",version=f"FinderView Terminal {__version__}")
    parser.add_argument("--event-log",type=Path,help=argparse.SUPPRESS)
    args=parser.parse_args()
    root=Path(args.path).expanduser().resolve()
    if not root.is_dir():
        parser.error(f"Not a directory: {root}")
    try:
        FinderViewApp(root,event_log=args.event_log).run()
    except (OSError,ValueError) as error:
        print(f"FinderView: {error}",file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
