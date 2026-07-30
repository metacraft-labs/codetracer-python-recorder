"""WSGI middleware recording each HTTP request as a span in the container.

Wraps a WSGI application (Flask, Django, or anything WSGI-compliant) so every
request becomes an **inline-bound** ``web-request`` span in the ``.ct`` container
the recorder is writing: HTTP metadata *plus* the step range the request
executed, which is what lets CodeTracer's Request Panel seek from a row to that
request's handler.  Before RS-M5 this middleware wrote a
``codetracer_spans.jsonl`` sidecar with no link to any trace; RS-M12 removed
that writer entirely — see ``span_recorder.py``.

The span lifecycle, the step-range capture and the concurrency argument live in
:mod:`codetracer_python_recorder.middleware.span_recorder`.

Usage:
    # Flask
    app.wsgi_app = CodeTracerWSGIMiddleware(app.wsgi_app, framework="flask")

    # Django (in wsgi.py)
    application = CodeTracerWSGIMiddleware(application, framework="django")

Nothing needs to be conditional on whether a recording is active: with no
session the middleware allocates ids and returns, and no span is written.
"""

from __future__ import annotations

from typing import Optional

from .span_recorder import RequestSpanRecorder, parse_status_code


class CodeTracerWSGIMiddleware:
    """Record one span per request handled by the wrapped WSGI app.

    ``framework`` is recorded as the span's ``framework`` metadata value.
    ``concurrent`` should be set when the server dispatches requests to worker
    threads, so overlapping step ranges are declared rather than implied.

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
        concurrent: bool = False,
        publish_open: bool = True,
    ) -> None:
        self.app = app
        self.spans = RequestSpanRecorder(
            framework=framework,
            concurrent=concurrent,
            publish_open=publish_open,
        )

    def __call__(self, environ, start_response):
        method = environ.get("REQUEST_METHOD", "GET")
        path = environ.get("PATH_INFO", "/")
        query = environ.get("QUERY_STRING", "")
        url = f"{path}?{query}" if query else path
        remote_addr = environ.get("REMOTE_ADDR", "")

        pending = self.spans.begin(method, url, remote_addr)

        captured_status = [None]
        captured_length = [None]

        def custom_start_response(status, headers, exc_info=None):
            captured_status[0] = status
            for name, value in headers or ():
                if name.lower() == "content-length":
                    try:
                        captured_length[0] = int(value)
                    except (TypeError, ValueError):
                        captured_length[0] = None
            return start_response(status, headers, exc_info)

        try:
            result = self.app(environ, custom_start_response)
        except Exception as exc:
            # The app raised past the middleware: WSGI servers turn that into a
            # 500, so the span records one, with the exception as
            # ``error.message`` (the well-known key for a failed span).
            pending.finish(
                500,
                route=route_of(environ),
                error_message=f"{type(exc).__name__}: {exc}",
            )
            raise

        status_code = parse_status_code(captured_status[0])
        body_size = captured_length[0]
        if body_size is None and isinstance(result, (list, tuple)):
            # No Content-Length header: measure the body only when the app handed
            # back a materialised sequence.  Iterating a generator here would
            # consume the response, so a streaming response reports no size
            # rather than a wrong one (``http.response_size`` is optional).
            body_size = sum(len(chunk) for chunk in result)
        pending.finish(
            status_code,
            response_size=body_size,
            route=route_of(environ),
            error_message=error_message_of(environ, status_code),
        )
        return result


def route_of(environ) -> Optional[str]:
    """The framework's route pattern for this request, when it exposes one.

    ``http.route`` exists to group requests that differ only in their path
    parameters (``/api/users/42`` and ``/api/users/7`` share
    ``/api/users/<int:user_id>``).  WSGI defines no slot for it, so each
    framework publishes it into the environ:

    * Werkzeug/Flask put the matched ``Rule`` in ``environ['werkzeug.rule']``
      when routing succeeded; its ``.rule`` attribute is the pattern.
    * Django's URL resolver is not exposed through WSGI at all, so the Django
      integration publishes ``environ['codetracer.route']`` from a Django-level
      middleware, where ``request.resolver_match.route`` is available.

    Returns ``None`` for an unrouted request (a 404 never matched a pattern), in
    which case the optional key is simply omitted from the span.
    """
    for key in ("codetracer.route", "werkzeug.rule", "REQUEST_ROUTE"):
        value = environ.get(key)
        if value:
            rule = getattr(value, "rule", None)
            return str(rule) if rule else str(value)
    return None


def error_message_of(environ, status_code: int) -> Optional[str]:
    """A framework-supplied error description for a failed request, if any.

    A framework that handled its own exception (Flask's error handler turning a
    raised ``RuntimeError`` into a 500 response) can publish the message under
    ``environ['codetracer.error']``; the middleware never sees the exception in
    that case, because the app returned normally.
    """
    if status_code < 500:
        return None
    message = environ.get("codetracer.error")
    return str(message) if message else None
