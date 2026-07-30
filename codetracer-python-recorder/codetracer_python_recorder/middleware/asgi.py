"""ASGI middleware recording each HTTP request as a span in the container.

The async counterpart of :mod:`codetracer_python_recorder.middleware.wsgi`: it
wraps an ASGI application (FastAPI, Starlette) so every request becomes an
inline-bound ``web-request`` span carrying both its HTTP metadata and the step
range it executed.

## Why the async case needs no extra machinery

Async handlers interleave: several requests are in flight on the SAME thread,
each suspended at an ``await``.  All per-request state therefore lives on the
:class:`~codetracer_python_recorder.middleware.span_recorder.PendingRequestSpan`
held in this coroutine's frame — there is no "current span" global and no
thread-local, either of which would have merged concurrent requests into one.

Their step ranges genuinely OVERLAP, because the recorder writes one step
timeline per process and the interleaved handlers all write into it.  Spans say
so: an ASGI span is marked ``concurrent_with_siblings`` and NOT
``contiguous_on_one_thread``, which is the format's way of telling a UI that the
range is not a continuous call trace (Trace-Spans.md § 2.4).

Usage:
    # FastAPI / Starlette (outermost, so the span covers the whole stack)
    app = CodeTracerASGIMiddleware(app, framework="fastapi")
"""

from __future__ import annotations

from typing import Optional

from .span_recorder import RequestSpanRecorder


class CodeTracerASGIMiddleware:
    """Record one span per HTTP request handled by the wrapped ASGI app.

    Non-HTTP scopes (``lifespan``, ``websocket``) pass straight through: they are
    not requests and have no ``http.*`` metadata to record.

    RS-M12 removed the ``manifest_path`` parameter along with the sidecar
    writer.  It was the SECOND POSITIONAL argument, so a caller that still
    passes it now gets a ``TypeError`` rather than silently configuring
    nothing.
    """

    def __init__(
        self,
        app,
        *,
        framework: str = "",
        publish_open: bool = True,
    ) -> None:
        self.app = app
        self.spans = RequestSpanRecorder(
            framework=framework,
            # Always true for ASGI: handlers interleave by construction, so
            # sibling spans may overlap in step space even on one thread.
            concurrent=True,
            publish_open=publish_open,
        )

    async def __call__(self, scope, receive, send):
        if scope.get("type") != "http":
            await self.app(scope, receive, send)
            return

        method = scope.get("method", "GET")
        path = scope.get("path", "/")
        query = scope.get("query_string", b"") or b""
        if isinstance(query, bytes):
            query = query.decode("latin-1")
        url = f"{path}?{query}" if query else path
        client = scope.get("client") or ()
        remote_addr = str(client[0]) if client else ""

        pending = self.spans.begin(method, url, remote_addr)

        captured_status = [0]
        captured_length = [None]
        body_bytes = [0]

        async def send_wrapper(message):
            message_type = message.get("type")
            if message_type == "http.response.start":
                captured_status[0] = message.get("status", 0)
                for name, value in message.get("headers", ()) or ():
                    if bytes(name).lower() == b"content-length":
                        try:
                            captured_length[0] = int(bytes(value))
                        except (TypeError, ValueError):
                            captured_length[0] = None
            elif message_type == "http.response.body":
                # Count what actually goes out, so a streaming response still
                # reports a size (unlike WSGI, where consuming the iterable to
                # measure it would break the response).
                body_bytes[0] += len(message.get("body", b"") or b"")
            await send(message)

        try:
            await self.app(scope, receive, send_wrapper)
        except Exception as exc:
            # Starlette's ServerErrorMiddleware normally converts this into a
            # 500 response before it reaches here; when it does not, the span
            # still records the failure rather than staying open forever.
            pending.finish(
                captured_status[0] or 500,
                route=route_of(scope),
                error_message=f"{type(exc).__name__}: {exc}",
            )
            raise

        size = captured_length[0]
        if size is None:
            size = body_bytes[0]
        pending.finish(
            captured_status[0],
            response_size=size,
            route=route_of(scope),
        )


def route_of(scope) -> Optional[str]:
    """The matched route pattern for this request, when the framework set one.

    Starlette (and therefore FastAPI) puts the matched ``APIRoute`` /
    ``Route`` object in ``scope['route']`` once routing has run; its ``path``
    attribute is the pattern (``/api/users/{user_id}``).  A middleware wrapped
    OUTSIDE the router sees the scope after the router mutated it, so this is
    read at request end, not at request start.

    Returns ``None`` when routing did not match (a 404 has no route).
    """
    route = scope.get("route")
    if route is None:
        return None
    pattern = getattr(route, "path", None) or getattr(route, "path_format", None)
    return str(pattern) if pattern else None
