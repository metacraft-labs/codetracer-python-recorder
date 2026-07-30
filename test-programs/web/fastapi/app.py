"""FastAPI demo app for the CodeTracer Request Panel (RS-M5).

The async counterpart of the Flask demo.  Two things it exists to show that a
WSGI app cannot:

* ``/api/slow/{token}`` awaits, so several requests are genuinely IN FLIGHT at
  the same time on one thread.  Their recorded step ranges therefore overlap,
  and their spans declare that (``concurrent_with_siblings``, and NOT
  ``contiguous_on_one_thread``).
* ``scope['route']`` gives ``http.route`` the FastAPI path pattern
  (``/api/users/{user_id}``) rather than the concrete URL.

The middleware wraps the app object itself (outermost), so a span covers
routing, dependency resolution and Starlette's error handling.
"""

from __future__ import annotations

import asyncio

from fastapi import FastAPI
from fastapi.responses import JSONResponse

from codetracer_python_recorder.middleware.asgi import CodeTracerASGIMiddleware

USERS = {1: {"id": 1, "name": "Alice"}, 2: {"id": 2, "name": "Bob"}}


def build_fastapi_app() -> FastAPI:
    """The bare FastAPI app, without the recorder middleware."""
    app = FastAPI()

    @app.get("/api/users")
    async def list_users():
        return sorted(USERS.values(), key=lambda user: user["id"])

    @app.get("/api/users/{user_id}")
    async def get_user(user_id: int):
        user = USERS.get(user_id)
        if user is None:
            return JSONResponse({"error": "not found"}, status_code=404)
        return user

    @app.get("/api/slow/{token}")
    async def slow(token: str, ms: int = 60):
        # The await is the point: while this request is suspended the event loop
        # runs OTHER requests' handlers, so the recorded steps of concurrent
        # requests interleave and their span step ranges overlap.
        await asyncio.sleep(ms / 1000)
        return {"token": token, "waited_ms": ms}

    @app.get("/api/boom")
    async def boom():
        raise RuntimeError("async handler raised on purpose")

    return app


def build_app(concurrent: bool = True):
    """The middleware-wrapped ASGI application the harness serves.

    ``concurrent`` is accepted for signature parity with the WSGI demos and is
    ignored: ASGI handlers interleave by construction, so their spans always
    declare that they may overlap a sibling's step range.
    """
    del concurrent
    return CodeTracerASGIMiddleware(build_fastapi_app(), framework="fastapi")
