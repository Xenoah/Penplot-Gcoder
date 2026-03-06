"""Application entry point — creates QApplication and shows the main window."""
from __future__ import annotations
import sys
from PyQt6.QtWidgets import QApplication
from PyQt6.QtCore import Qt
from src.ui.main_window import MainWindow


def run():
    """Initialise and run the Qt application."""
    app = QApplication.instance() or QApplication(sys.argv)
    app.setApplicationName("Penplot-Gcoder")
    app.setOrganizationName("PenplotGcoder")

    # High DPI support (Qt6 handles this automatically, but be explicit)
    app.setHighDpiScaleFactorRoundingPolicy(
        Qt.HighDpiScaleFactorRoundingPolicy.PassThrough
    )

    window = MainWindow()
    window.show()

    sys.exit(app.exec())
