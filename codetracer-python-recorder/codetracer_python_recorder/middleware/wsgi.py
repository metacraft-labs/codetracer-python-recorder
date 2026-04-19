"""WSGI middleware for CodeTracer span tracking.

Wraps each HTTP request in a CodeTracer span with method, URL, status
code, and duration metadata. Compatible with Flask, Django, and any
WSGI-compliant framework.

Usage:
    # Flask
    app.wsgi_app = CodeTracerWSGIMiddleware(app.wsgi_app)

    # Django (in wsgi.py)
    application = CodeTracerWSGIMiddleware(application)
"""

import json
import os
import time
import threading
from datetime import datetime, timezone


class CodeTracerWSGIMiddleware:
    def __init__(self, app, manifest_path=None):
        self.app = app
        self.manifest_path = manifest_path or os.environ.get(
            'CODETRACER_SPAN_MANIFEST', '/tmp/codetracer_spans.jsonl'
        )
        self._lock = threading.Lock()
        self._span_counter = 0

    def __call__(self, environ, start_response):
        method = environ.get('REQUEST_METHOD', 'GET')
        path = environ.get('PATH_INFO', '/')
        start_time = time.monotonic()
        span_id = self._generate_span_id()

        captured_status = [None]

        def custom_start_response(status, headers, exc_info=None):
            captured_status[0] = status
            return start_response(status, headers, exc_info)

        try:
            result = self.app(environ, custom_start_response)
            duration_ms = int((time.monotonic() - start_time) * 1000)
            status_code = self._parse_status_code(captured_status[0])
            self._write_span(span_id, method, path, status_code, duration_ms)
            return result
        except Exception:
            duration_ms = int((time.monotonic() - start_time) * 1000)
            self._write_span(span_id, method, path, 500, duration_ms, status='error')
            raise

    def _generate_span_id(self):
        with self._lock:
            self._span_counter += 1
            return f"span_wsgi_{self._span_counter}"

    def _parse_status_code(self, status_str):
        if status_str is None:
            return 0
        try:
            return int(str(status_str).split(' ')[0])
        except (ValueError, IndexError):
            return 0

    def _write_span(self, span_id, method, path, status_code, duration_ms,
                    status=None):
        span = {
            'id': span_id,
            'label': f'{method} {path}',
            'span_type': 'web-request',
            'metadata': {
                'http.method': method,
                'http.url': path,
                'http.status_code': str(status_code),
                'http.duration_ms': str(duration_ms),
            },
            'status': status or ('error' if status_code >= 400 else 'ok'),
            'end_time': datetime.now(timezone.utc).isoformat(),
        }
        try:
            with self._lock:
                with open(self.manifest_path, 'a') as f:
                    f.write(json.dumps(span) + '\n')
        except OSError:
            pass  # Don't crash the app
