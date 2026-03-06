"""Penplot-Gcoder — entry point."""
import sys
import os

# Ensure project root is on sys.path so that `src` is importable
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from src.app import run

if __name__ == "__main__":
    run()
