"""Penplot-Gcoder — entry point with auto dependency installation."""
import sys
import os
import subprocess

# ---------------------------------------------------------------------------
# Auto-install missing packages before any heavy imports
# ---------------------------------------------------------------------------
_REQUIREMENTS = [
    ("PyQt6",          "PyQt6>=6.4.0"),
    ("pyqtgraph",      "pyqtgraph>=0.13.0"),
    ("numpy",          "numpy>=1.24.0"),
    ("shapely",        "Shapely>=2.0.0"),
    ("svgpathtools",   "svgpathtools>=1.6.0"),
    ("ezdxf",          "ezdxf>=1.0.0"),
    ("cv2",            "opencv-python>=4.8.0"),
]


def _check_and_install():
    missing = []
    for import_name, pip_spec in _REQUIREMENTS:
        try:
            __import__(import_name)
        except ImportError:
            missing.append(pip_spec)

    if not missing:
        return  # all good

    print("=" * 60)
    print("Penplot-Gcoder: 不足しているパッケージをインストールします...")
    print("Missing packages:")
    for pkg in missing:
        print(f"  - {pkg}")
    print("=" * 60)

    try:
        subprocess.check_call(
            [sys.executable, "-m", "pip", "install"] + missing,
            stdout=sys.stdout,
            stderr=sys.stderr,
        )
        print("\nインストール完了。アプリを起動します...\n")
    except subprocess.CalledProcessError as e:
        print(f"\n[ERROR] パッケージのインストールに失敗しました: {e}")
        print("手動で以下を実行してください:")
        print(f"  pip install {' '.join(missing)}")
        input("\nEnterキーを押すと終了します...")
        sys.exit(1)


_check_and_install()

# ---------------------------------------------------------------------------
# Ensure project root is on sys.path so that `src` is importable
# ---------------------------------------------------------------------------
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from src.app import run

if __name__ == "__main__":
    run()
