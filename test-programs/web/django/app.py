"""Django demo app for the CodeTracer Request Panel (RS-M5).

A whole Django project in one module: ``settings.configure()`` instead of a
generated project tree, because the point of the demo is the request path, not
Django's layout.  Views are plain functions and the URLconf is this module, so a
request's recorded step range is exactly its view.

Django is the reason ``http.route`` exists in the well-known metadata set: its
URL resolver knows the route pattern (``api/users/<int:user_id>``) that a
concrete URL matched, which is what groups requests in the panel.  Django does
not expose the resolver through WSGI, so :func:`publish_route_middleware` copies
``request.resolver_match.route`` into the WSGI environ where the recorder's
middleware reads it.
"""

from __future__ import annotations

import json
import time

from django.conf import settings
from django.http import JsonResponse
from django.urls import path

from codetracer_python_recorder.middleware.wsgi import CodeTracerWSGIMiddleware

USERS = {1: {"id": 1, "name": "Alice"}, 2: {"id": 2, "name": "Bob"}}


def list_users(request):
    return JsonResponse(sorted(USERS.values(), key=lambda user: user["id"]), safe=False)


def create_user(request):
    payload = json.loads(request.body or b"{}")
    new_id = max(USERS) + 1
    USERS[new_id] = {"id": new_id, "name": payload.get("name", "anonymous")}
    return JsonResponse(USERS[new_id], status=201)


def get_user(request, user_id: int):
    user = USERS.get(user_id)
    if user is None:
        return JsonResponse({"error": "not found"}, status=404)
    return JsonResponse(user)


def slow_report(request):
    time.sleep(0.05)
    return JsonResponse({"rows": [1, 2, 3]})


def boom(request):
    raise RuntimeError("view raised on purpose")


urlpatterns = [
    path("api/users", list_users),
    path("api/users/new", create_user),
    path("api/users/<int:user_id>", get_user),
    path("api/reports/slow", slow_report),
    path("api/boom", boom),
]


def publish_route_middleware(get_response):
    """Django middleware copying the resolved route pattern into the environ.

    ``request.resolver_match`` is populated by the URL resolver before the view
    runs, so by the time the response comes back the route is known.  The
    recorder's WSGI middleware sits outside Django and reads
    ``environ['codetracer.route']``.
    """

    def middleware(request):
        response = get_response(request)
        match = getattr(request, "resolver_match", None)
        if match is not None and getattr(match, "route", None):
            request.META["codetracer.route"] = str(match.route)
        return response

    return middleware


def _configure() -> None:
    """Configure Django in-process (idempotent)."""
    if settings.configured:
        return
    settings.configure(
        DEBUG=False,
        # `raise_request_exception` behaviour: with DEBUG off and no
        # ALLOWED_HOSTS wildcard Django would answer 400 to the test client's
        # Host header, so the loopback names the harness serves on are listed.
        ALLOWED_HOSTS=["127.0.0.1", "localhost", "testserver"],
        SECRET_KEY="codetracer-request-panel-demo",
        ROOT_URLCONF=__name__,
        MIDDLEWARE=[f"{__name__}.publish_route_middleware"],
        DATABASES={},
        # Silence Django's logging config so the demo's stdout stays readable;
        # the 500 from `/api/boom` is expected output, not a failure.
        LOGGING_CONFIG=None,
        USE_TZ=True,
    )


def build_django_app():
    """The bare Django WSGI application, without the recorder middleware."""
    import django
    from django.core.wsgi import get_wsgi_application

    _configure()
    django.setup()
    return get_wsgi_application()


def build_app(concurrent: bool = False):
    """The middleware-wrapped WSGI application the harness serves.

    See the Flask demo's ``build_app`` for ``concurrent``.
    """
    return CodeTracerWSGIMiddleware(build_django_app(), framework="django", concurrent=concurrent)
