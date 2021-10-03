# mc-proxy
This is a fully functional proxy for online and offline-mode Minecraft servers. It is based on an optimized, custom-made packet parser for Minecraft 1.8+ (tested up to 1.16), and minimizes memory usage by parsing the packets in a streaming fashion rather than loading them into memory all at once.

The codebase contains both a client and a server. Packets received by the server are forwarded to the client, and vice versa, but packets can be intercepted and/or rewritten dynamically. The server does not support compression or online-mode and there are no plans to add this, since it would only reduce performance, though implementing it is possible if desired. Although the client does not currently supports only Mojang authentication (accounts migrated to Microsoft will not work), adding support for Microsoft is planned.

The default implementation in `main.rs` adds a stat checker for Hypixel. The binary must currently be called with your Minecraft username, password, and Hypixel API key as command line arguments (very secure, I know).

The main purpose of this particular project is proxying and adding custom command support, but it is generic enough to support a wide variety of network-level plugins, which work on any Minecraft client. Since this codebase implements packet parsing and authentication from scratch, feel free to fork it and do whatever you want with the core architecture. 

## TODOs
- Improve performance - buffering the TCP streams could be useful
- Make more extensible - add an on-the-fly command creation system
- Add a GUI for server selection
- Fix bugs (there are a lot)
- Fix warnings (there are even more)