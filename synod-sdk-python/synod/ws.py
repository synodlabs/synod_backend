"""Synod Agent SDK — WebSocket client with automatic reconnection.

Handles exponential backoff (1s base, 30s cap) and state resync on reconnect.
"""

import asyncio
import json
import logging
from typing import Callable, Any

import websockets
from websockets.exceptions import ConnectionClosed

logger = logging.getLogger("synod.ws")


class SynodWebSocket:
    """Persistent WebSocket connection to the coordinator event stream."""

    def __init__(
        self,
        url: str,
        on_event: Callable[[dict], Any],
        on_disconnect: Callable[[], Any] | None = None,
    ):
        self._url = url
        self._on_event = on_event
        self._on_disconnect = on_disconnect
        self._ws = None
        self._task: asyncio.Task | None = None
        self._running = False
        self._consecutive_failures = 0
        self._max_backoff = 30.0
        self._base_backoff = 1.0
        self._unreachable_since: float | None = None

    async def start(self) -> None:
        """Start the WebSocket connection loop."""
        self._running = True
        self._task = asyncio.create_task(self._run_loop())

    def update_url(self, url: str) -> None:
        """Update the connection URL used for future reconnect attempts."""
        self._url = url

    async def stop(self) -> None:
        """Gracefully stop the WebSocket connection."""
        self._running = False
        if self._ws:
            await self._ws.close()
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass

    async def _run_loop(self) -> None:
        """Main reconnection loop with exponential backoff."""
        while self._running:
            try:
                async with websockets.connect(self._url) as ws:
                    self._ws = ws
                    self._consecutive_failures = 0
                    self._unreachable_since = None
                    logger.info("WebSocket connected to %s", self._url)

                    # Start heartbeat
                    heartbeat_task = asyncio.create_task(self._heartbeat(ws))

                    try:
                        async for message in ws:
                            try:
                                data = json.loads(message)
                                await self._on_event(data)
                            except json.JSONDecodeError:
                                if message == "pong":
                                    continue
                                logger.warning("Non-JSON message: %s", message[:100])
                    except ConnectionClosed:
                        logger.info("WebSocket connection closed")
                    finally:
                        heartbeat_task.cancel()
                        try:
                            await heartbeat_task
                        except asyncio.CancelledError:
                            pass

            except Exception as e:
                self._consecutive_failures += 1
                if self._unreachable_since is None:
                    self._unreachable_since = asyncio.get_event_loop().time()
                
                logger.warning(
                    "WebSocket connection failed (attempt %d): %s",
                    self._consecutive_failures, e
                )

                # Check if unreachable for more than 5 minutes
                if self._unreachable_since is not None:
                    elapsed = asyncio.get_event_loop().time() - self._unreachable_since
                    if elapsed > 300:
                        logger.error("Coordinator unreachable for >5 minutes")
                        if self._on_disconnect:
                            await self._on_disconnect()

            if self._running:
                delay = min(
                    self._base_backoff * (2 ** self._consecutive_failures),
                    self._max_backoff
                )
                logger.info("Reconnecting in %.1fs...", delay)
                await asyncio.sleep(delay)

    async def _heartbeat(self, ws) -> None:
        """Send ping every 60 seconds to keep the connection alive."""
        while True:
            await asyncio.sleep(60)
            try:
                await ws.send("ping")
            except Exception:
                break
