"""Background pipeline worker — runs heavy processing off the UI thread.

Usage
-----
    worker = PipelineWorker(job_id, task_fn, parent=self)
    worker.result_ready.connect(self._on_result)
    worker.error_occurred.connect(self._on_error)
    worker.progress.connect(self._on_progress)
    worker.start()

task_fn signature
-----------------
    def task_fn(is_cancelled: Callable[[], bool]) -> tuple[list, bool, str]:
        # returns (display_groups, overflow, gcode_text)

Cancel safety
-------------
Call worker.cancel() to request cancellation.
The task_fn should poll is_cancelled() and raise if True, or just return early.
Stale results are filtered by job_id in the receiver.
"""
from __future__ import annotations

from typing import Any, Callable, Tuple

from PyQt6.QtCore import QThread, pyqtSignal


class PipelineWorker(QThread):
    """QThread that executes a pipeline task function in the background.

    Signals
    -------
    result_ready(job_id, display_groups, overflow, gcode_text)
    error_occurred(job_id, message)
    progress(message)
    """

    result_ready  = pyqtSignal(int, object, bool, str)   # job_id, groups, overflow, gcode
    error_occurred = pyqtSignal(int, str)                 # job_id, message
    progress      = pyqtSignal(str)

    def __init__(self, job_id: int, task_fn: Callable, parent=None):
        super().__init__(parent)
        self._job_id    = job_id
        self._task_fn   = task_fn
        self._cancelled = False

    @property
    def job_id(self) -> int:
        return self._job_id

    def cancel(self) -> None:
        """Request cancellation. The task_fn must honour is_cancelled()."""
        self._cancelled = True

    def is_cancelled(self) -> bool:
        return self._cancelled

    # ──────────────────────────────────────────────────────────────────────
    def run(self) -> None:
        try:
            result = self._task_fn(self.is_cancelled, self.progress.emit)
            if not self._cancelled and result is not None:
                groups, overflow, gcode = result
                self.result_ready.emit(self._job_id, groups, overflow, gcode)
        except _CancelledError:
            pass  # silently drop cancelled jobs
        except Exception as exc:  # noqa: BLE001
            if not self._cancelled:
                self.error_occurred.emit(self._job_id, str(exc))


class _CancelledError(Exception):
    """Raised by task helpers when cancellation is detected."""
