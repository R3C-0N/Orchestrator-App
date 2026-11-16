"""Réglages du client de démonstration."""

HOST = "localhost"
PORT = 8080

TIMEOUT = 30
RETRIES = 3

LOG_LEVEL = "info"


def url() -> str:
    return f"http://{HOST}:{PORT}"
