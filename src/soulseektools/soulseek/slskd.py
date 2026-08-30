"""The slskd backend.

Every request and response shape here was pinned against a running
``slskd/slskd`` container's OpenAPI document and its bundled web UI, not from
memory; see ``docs/slskd-setup.md``. Keeping the wire format in this one module
means a change to slskd is a change to one file.

Authentication is the ``X-API-Key`` header. The key never comes from the config
file -- see :mod:`soulseektools.config`.
"""

from __future__ import annotations

import time
import uuid
from typing import TYPE_CHECKING, Any

import httpx

from soulseektools.errors import SoulseekClientError
from soulseektools.models import Candidate
from soulseektools.soulseek.base import Transfer, TransferState

if TYPE_CHECKING:
    from collections.abc import Sequence

    from soulseektools.config import SlskdSettings

API_PREFIX = "/api/v0"
API_KEY_HEADER = "X-API-Key"

_POLL_INTERVAL_SECONDS = 1.0
_SEARCH_GRACE_SECONDS = 5.0

# slskd reports the underlying Soulseek.NET transfer states, which are flag-like
# strings such as "Completed, Succeeded" or "Completed, Errored".
_TERMINAL_STATES = {
    "succeeded": TransferState.COMPLETED,
    "cancelled": TransferState.CANCELLED,
    "errored": TransferState.FAILED,
    "timedout": TransferState.FAILED,
    "rejected": TransferState.FAILED,
}


class SlskdClient:
    """Talks to a slskd daemon over its REST API."""

    def __init__(self, settings: SlskdSettings, client: httpx.Client | None = None) -> None:
        self.settings = settings
        self._client = client or self._build_client(settings)

    @staticmethod
    def _build_client(settings: SlskdSettings) -> httpx.Client:
        headers = {"Accept": "application/json"}
        if settings.api_key is not None:
            headers[API_KEY_HEADER] = settings.api_key.get_secret_value()
        return httpx.Client(
            base_url=settings.url.rstrip("/"),
            headers=headers,
            timeout=settings.request_timeout_seconds,
        )

    def close(self) -> None:
        self._client.close()

    def __enter__(self) -> SlskdClient:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def is_connected(self) -> bool:
        """Whether slskd is logged in to the Soulseek server."""
        try:
            state = self._get("/server")
        except SoulseekClientError:
            return False
        return bool(state.get("isConnected") and state.get("isLoggedIn"))

    def search(
        self, text: str, *, timeout_seconds: int = 15, response_limit: int = 200
    ) -> list[Candidate]:
        """Run one search, wait for slskd to finish it, and flatten the responses."""
        search_id = str(uuid.uuid4())
        self._post(
            "/searches",
            {
                "id": search_id,
                "searchText": text,
                "searchTimeout": max(timeout_seconds, 5),
                "responseLimit": response_limit,
            },
        )

        try:
            self._await_completion(search_id, timeout_seconds)
            responses = self._get(f"/searches/{search_id}/responses")
        finally:
            self._delete(f"/searches/{search_id}")

        return [
            candidate
            for response in _as_list(responses)
            for candidate in _candidates_from(response)
        ]

    def enqueue(
        self, username: str, files: Sequence[Candidate], *, destination: str = ""
    ) -> list[Transfer]:
        """Queue files from one peer, optionally into a named destination folder."""
        if not files:
            return []

        items = [{"filename": candidate.remote_path, "size": candidate.size} for candidate in files]
        if destination:
            payload = {
                "username": username,
                "files": items,
                "options": {"destination": destination},
            }
            response = self._post("/transfers/downloads/batches", payload)
            failures = _as_list(response.get("failures"))
            if failures:
                raise SoulseekClientError(
                    "; ".join(
                        f"{failure.get('filename')}: {failure.get('message')}"
                        for failure in failures
                    )
                )
            batch = response.get("batch") or {}
            return [_transfer_from(item) for item in _as_list(batch.get("transfers"))]

        self._post(f"/transfers/downloads/{username}", items)
        return [
            transfer
            for transfer in self.transfers()
            if transfer.username == username
            and transfer.remote_path in {candidate.remote_path for candidate in files}
        ]

    def transfers(self) -> list[Transfer]:
        """Every download slskd is tracking, across all peers."""
        return [
            _transfer_from(item, username=user.get("username", ""))
            for user in _as_list(self._get("/transfers/downloads"))
            for directory in _as_list(user.get("directories"))
            for item in _as_list(directory.get("files"))
        ]

    def _await_completion(self, search_id: str, timeout_seconds: int) -> None:
        deadline = time.monotonic() + timeout_seconds + _SEARCH_GRACE_SECONDS
        while time.monotonic() < deadline:
            state = self._get(f"/searches/{search_id}")
            if state.get("isComplete"):
                return
            time.sleep(_POLL_INTERVAL_SECONDS)

    def _get(self, path: str) -> Any:
        return self._request("GET", path)

    def _post(self, path: str, payload: Any) -> Any:
        return self._request("POST", path, json=payload)

    def _delete(self, path: str) -> Any:
        try:
            return self._request("DELETE", path)
        except SoulseekClientError:
            # A search that slskd has already cleaned up is not a failure worth
            # surfacing over the results it produced.
            return None

    def _request(self, method: str, path: str, **kwargs: Any) -> Any:
        try:
            response = self._client.request(method, f"{API_PREFIX}{path}", **kwargs)
            response.raise_for_status()
        except httpx.HTTPStatusError as error:
            raise SoulseekClientError(
                f"slskd returned {error.response.status_code} for {method} {path}"
                + (": check SLSKD_API_KEY" if error.response.status_code == 401 else "")
            ) from error
        except httpx.HTTPError as error:
            raise SoulseekClientError(
                f"cannot reach slskd at {self.settings.url}: {error}"
            ) from error

        if not response.content:
            return None
        try:
            return response.json()
        except ValueError:
            return None


def _candidates_from(response: dict[str, Any]) -> list[Candidate]:
    username = str(response.get("username", ""))
    free_slots = bool(response.get("hasFreeUploadSlot"))
    queue_length = int(response.get("queueLength") or 0)

    return [
        Candidate(
            username=username,
            remote_path=str(file.get("filename", "")),
            size=int(file.get("size") or 0),
            bitrate=_optional_int(file.get("bitRate")),
            sample_rate=_optional_int(file.get("sampleRate")),
            bit_depth=_optional_int(file.get("bitDepth")),
            is_vbr=bool(file.get("isVariableBitRate")),
            free_upload_slots=free_slots,
            queue_length=queue_length,
            duration_seconds=_optional_float(file.get("length")),
        )
        for file in _as_list(response.get("files"))
        if file.get("filename")
    ]


def _transfer_from(item: dict[str, Any], username: str = "") -> Transfer:
    return Transfer(
        id=str(item.get("id", "")),
        username=str(item.get("username") or username),
        remote_path=str(item.get("filename", "")),
        state=_state_from(str(item.get("state", ""))),
        size=int(item.get("size") or 0),
        bytes_transferred=int(item.get("bytesTransferred") or 0),
        place_in_queue=_optional_int(item.get("placeInQueue")),
        error=str(item.get("exception") or ""),
    )


def _state_from(raw: str) -> TransferState:
    """Collapse a comma-separated Soulseek.NET state into one outcome."""
    parts = {part.strip().lower() for part in raw.split(",")}
    for name, state in _TERMINAL_STATES.items():
        if name in parts:
            return state
    if "completed" in parts:
        return TransferState.COMPLETED
    if "inprogress" in parts:
        return TransferState.IN_PROGRESS
    return TransferState.QUEUED


def _as_list(value: Any) -> list[Any]:
    return list(value) if isinstance(value, list) else []


def _optional_int(value: Any) -> int | None:
    try:
        number = int(value)
    except (TypeError, ValueError):
        return None
    return number or None


def _optional_float(value: Any) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number or None
