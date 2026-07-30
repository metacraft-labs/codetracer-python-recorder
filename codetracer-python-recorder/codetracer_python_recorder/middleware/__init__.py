"""HTTP middleware that records each request as a span in the trace container.

Since RS-M5 both middlewares write ``web-request`` spans into the ``.ct``
container the recorder is producing (``spans.dat``), so a request row in
CodeTracer's Request Panel can seek into that request's handler.  RS-M12
removed the legacy ``codetracer_spans.jsonl`` sidecar writer entirely; nothing
here writes a file, and ``CODETRACER_SPAN_MANIFEST`` is no longer read.

* :class:`CodeTracerWSGIMiddleware` — Flask, Django, any WSGI app.
* :class:`CodeTracerASGIMiddleware` — FastAPI, Starlette, any ASGI app.

The shared lifecycle lives in
:mod:`codetracer_python_recorder.middleware.span_recorder`, which is also where
to read about where a span begins and ends and how concurrent requests stay
distinct.
"""

from .asgi import CodeTracerASGIMiddleware
from .span_recorder import PendingRequestSpan, RequestSpanRecorder
from .wsgi import CodeTracerWSGIMiddleware

__all__ = [
    "CodeTracerWSGIMiddleware",
    "CodeTracerASGIMiddleware",
    "RequestSpanRecorder",
    "PendingRequestSpan",
]
