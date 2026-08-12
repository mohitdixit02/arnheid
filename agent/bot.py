"""Entry point: runs the Telegram and Slack front ends concurrently, both
backed by agent_runner.run_turn. Each acks immediately, runs the agent turn
in the background, and posts the final result back to the originating
chat/thread — never blocks the event loop on a turn that might take minutes.

python bot.py
"""

from __future__ import annotations

import asyncio
import logging
import os
import time
from typing import Awaitable, Callable

from dotenv import load_dotenv

load_dotenv()

import agent_runner

logging.basicConfig(level=logging.INFO)
log = logging.getLogger("agent-bot")

Reply = Callable[[str], Awaitable[None]]

# chat_key -> monotonic start time of its in-flight turn, if any. One turn
# per chat at a time.
# ponytail: no queue — a message that arrives mid-turn just gets told to
# wait, rather than being queued or interleaved into the same session.
_busy: dict[str, float] = {}


async def handle_turn(chat_key: str, text: str, reply: Reply) -> None:
    if chat_key in _busy:
        elapsed = int(time.monotonic() - _busy[chat_key])
        await reply(f"Still working on the last thing ({elapsed}s so far) — hang tight.")
        return

    await reply("On it.")
    _busy[chat_key] = time.monotonic()
    try:
        result = await agent_runner.run_turn(chat_key, text)
        await reply(result.text or "Done — no output.")
    except Exception as e:  # noqa: BLE001 - must always reply, never go silent
        log.exception("agent turn failed for %s", chat_key)
        await reply(f"Hit an error running that: {e}")
    finally:
        _busy.pop(chat_key, None)


async def run_telegram() -> None:
    token = os.environ.get("TELEGRAM_BOT_TOKEN")
    if not token:
        log.info("TELEGRAM_BOT_TOKEN not set, skipping Telegram")
        return
    from telegram import Update
    from telegram.ext import Application, ContextTypes, MessageHandler, filters

    app = Application.builder().token(token).build()

    async def on_message(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
        if not update.message or not update.message.text:
            return
        chat_key = f"telegram:{update.effective_chat.id}"
        message = update.message

        async def reply(text: str) -> None:
            await message.reply_text(text)

        asyncio.create_task(handle_turn(chat_key, message.text, reply))

    app.add_handler(MessageHandler(filters.TEXT & ~filters.COMMAND, on_message))
    log.info("Telegram bot starting (long polling)")
    async with app:
        await app.start()
        await app.updater.start_polling()
        await asyncio.Event().wait()


async def run_slack() -> None:
    bot_token = os.environ.get("SLACK_BOT_TOKEN")
    app_token = os.environ.get("SLACK_APP_TOKEN")
    if not bot_token or not app_token:
        log.info("SLACK_BOT_TOKEN/SLACK_APP_TOKEN not set, skipping Slack")
        return
    from slack_bolt.adapter.socket_mode.async_handler import AsyncSocketModeHandler
    from slack_bolt.async_app import AsyncApp

    app = AsyncApp(token=bot_token)

    async def dispatch(channel: str, thread: str, text: str, say) -> None:
        if not text:
            return
        chat_key = f"slack:{channel}:{thread}"

        async def reply(msg: str) -> None:
            await say(text=msg, thread_ts=thread)

        asyncio.create_task(handle_turn(chat_key, text, reply))

    @app.event("app_mention")
    async def on_mention(event, say) -> None:
        thread = event.get("thread_ts") or event["ts"]
        await dispatch(event["channel"], thread, event.get("text", ""), say)

    @app.event("message")
    async def on_dm(event, say) -> None:
        # Only bare DMs, and never the bot echoing itself.
        if event.get("channel_type") != "im" or event.get("bot_id"):
            return
        thread = event.get("thread_ts") or event["ts"]
        await dispatch(event["channel"], thread, event.get("text", ""), say)

    log.info("Slack bot starting (Socket Mode)")
    handler = AsyncSocketModeHandler(app, app_token)
    await handler.start_async()


async def main() -> None:
    await asyncio.gather(run_telegram(), run_slack())


if __name__ == "__main__":
    asyncio.run(main())
