"""ASGI middleware for CodeTracer span tracking.

Wraps each HTTP request in a CodeTracer span. Supports async handlers
(FastAPI, Starlette).

Usage:
    # FastAPI
    app.add_middleware(CodeTracerASGIMiddleware)

    # Starlette
    app = CodeTracerASGIMiddleware(app)
"""

import json
import os
import time
import asyncio
from datetime import datetime, timezone


class CodeTracerASGIMiddleware:
    def __init__(self, app, manifest_path=None):
        self.app = app
        self.manifest_path = manifest_path or os.environ.get(
            'CODETRACER_SPAN_MANIFEST', '/tmp/codetracer_spans.jsonl'
        )
        self._span_counter = 0
        self._lock = asyncio.Lock() if hasattr(asyncio, 'Lock') else None

    async def __call__(self, scope, receive, send):
        if scope['type'] != 'http':
            await self.app(scope, receive, send)
            return

        method = scope.get('method', 'GET')
        path = scope.get('path', '/')
        start_time = time.monotonic()
        span_id = self._generate_span_id()
        captured_status = [None]

        async def send_wrapper(message):
            if message['type'] == 'http.response.start':
                captured_status[0] = message.get('status', 0)
            await send(message)

        try:
            await self.app(scope, receive, send_wrapper)
            duration_ms = int((time.monotonic() - start_time) * 1000)
            status_code = captured_status[0] or 0
            await self._write_span(span_id, method, path, status_code, duration_ms)
        except Exception:
            duration_ms = int((time.monotonic() - start_time) * 1000)
            await self._write_span(span_id, method, path, 500, duration_ms, status='error')
            raise

    def _generate_span_id(self):
        self._span_counter += 1
        return f"span_asgi_{self._span_counter}"

    async def _write_span(self, span_id, method, path, status_code, duration_ms,
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
            with open(self.manifest_path, 'a') as f:
                f.write(json.dumps(span) + '\n')
        except OSError:
            pass
