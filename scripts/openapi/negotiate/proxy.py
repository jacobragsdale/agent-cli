"""A proxy that only ever asks for Kerberos or NTLM, as a corporate proxy does.

Every request, CONNECT included, gets 407 with both challenges and the
connection closed, so a client can be seen to stop with a clear error
instead of looping or leaking a Basic credential.
"""

import asyncio

ANSWER = (
    b"HTTP/1.1 407 Proxy Authentication Required\r\n"
    b"Proxy-Authenticate: Negotiate\r\n"
    b"Proxy-Authenticate: NTLM\r\n"
    b"Content-Type: text/plain\r\n"
    b"Content-Length: 30\r\n"
    b"Connection: close\r\n"
    b"\r\n"
    b"proxy authentication required\n"
)


async def answer(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    try:
        await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), timeout=10)
        writer.write(ANSWER)
        await writer.drain()
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, TimeoutError, ConnectionError):
        pass
    finally:
        writer.close()


async def main() -> None:
    server = await asyncio.start_server(answer, "0.0.0.0", 3130)
    async with server:
        await server.serve_forever()


asyncio.run(main())
