"""Flask demo app for the CodeTracer Request Panel (RS-M5).

Every route is deterministic and every handler is short, so a request's recorded
step range is exactly the steps of its handler.  Between them the routes cover
each status bucket the Request Panel colours, a handler that raises (so a 500
span carries ``error.message``), and one deliberately slow handler so a duration
is visibly non-zero.

The middleware is installed on ``app.wsgi_app``, i.e. OUTSIDE Flask's own
request handling, so the span covers routing and error handling too.
"""

from __future__ import annotations

import time

from flask import Flask, jsonify, request

from codetracer_python_recorder.middleware.wsgi import CodeTracerWSGIMiddleware

USERS = {1: {"id": 1, "name": "Alice"}, 2: {"id": 2, "name": "Bob"}}


def build_flask_app() -> Flask:
    """The bare Flask app, without the recorder middleware."""
    app = Flask(__name__)

    @app.get("/api/users")
    def list_users():
        return jsonify(sorted(USERS.values(), key=lambda user: user["id"]))

    @app.post("/api/users")
    def create_user():
        payload = request.get_json(silent=True) or {}
        new_id = max(USERS) + 1
        USERS[new_id] = {"id": new_id, "name": payload.get("name", "anonymous")}
        return jsonify(USERS[new_id]), 201

    @app.get("/api/users/<int:user_id>")
    def get_user(user_id: int):
        user = USERS.get(user_id)
        if user is None:
            return jsonify({"error": "not found"}), 404
        return jsonify(user)

    @app.get("/static/app.css")
    def cached_asset():
        # A 3xx so the demo exercises every status bucket the Request Panel
        # colours.  304 rather than a redirect: an HTTP client follows a
        # redirect, which would add a second request the demo never asked for.
        return "", 304

    @app.get("/api/reports/slow")
    def slow_report():
        # Sleeps so `http.duration_ms` is unambiguously non-zero; the sleep is
        # inside the handler, hence inside the span's step range.
        time.sleep(0.05)
        return jsonify({"rows": [1, 2, 3]})

    @app.get("/api/boom")
    def boom():
        raise RuntimeError("handler raised on purpose")

    @app.after_request
    def publish_route(response):
        # Flask knows the matched rule (`/api/users/<int:user_id>`) but WSGI has
        # no standard slot for it, so publish it where the middleware looks.
        # `after_request` runs for error responses too, so a 500 keeps its route.
        if request.url_rule is not None:
            request.environ["codetracer.route"] = str(request.url_rule)
        return response

    @app.errorhandler(500)
    def on_server_error(error):
        # Flask converts the raised exception into a 500 response, so the
        # middleware never sees the exception.  Publishing the message into the
        # WSGI environ is how it still reaches the span's `error.message`.
        request.environ["codetracer.error"] = "handler raised on purpose"
        return jsonify({"error": "internal"}), 500

    return app


def build_app(concurrent: bool = False):
    """The middleware-wrapped WSGI application the harness serves.

    ``concurrent`` must be set when the server dispatches requests to worker
    threads, so overlapping step ranges are declared instead of implied.
    """
    app = build_flask_app()
    app.wsgi_app = CodeTracerWSGIMiddleware(app.wsgi_app, framework="flask", concurrent=concurrent)
    return app
