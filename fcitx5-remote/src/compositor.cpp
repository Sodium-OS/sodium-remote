// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "compositor.h"

#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <format>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace {

// Hyprland answers in well under a millisecond; never stall fcitx5's loop on it.
constexpr int timeoutMs = 100;

bool waitFor(int fd, short events) {
    pollfd p{fd, events, 0};
    return poll(&p, 1, timeoutMs) == 1 && (p.revents & events);
}

// One request on Hyprland's command socket, as hyprctl sends it.
std::optional<std::string> request(const std::string &command) {
    const char *runtime = std::getenv("XDG_RUNTIME_DIR");
    const char *instance = std::getenv("HYPRLAND_INSTANCE_SIGNATURE");
    if (!runtime || !instance) {
        return std::nullopt;
    }
    sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    auto path = std::format("{}/hypr/{}/.socket.sock", runtime, instance);
    if (path.size() >= sizeof(addr.sun_path)) {
        return std::nullopt;
    }
    std::memcpy(addr.sun_path, path.c_str(), path.size() + 1);

    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) {
        return std::nullopt;
    }
    std::optional<std::string> reply;
    auto message = "/" + command;
    if ((connect(fd, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) == 0 ||
         (errno == EINPROGRESS && waitFor(fd, POLLOUT))) &&
        write(fd, message.data(), message.size()) == static_cast<ssize_t>(message.size())) {
        std::string data;
        char buf[256];
        while (waitFor(fd, POLLIN)) {
            auto n = read(fd, buf, sizeof(buf));
            if (n <= 0) {
                break;
            }
            data.append(buf, n);
        }
        reply = std::move(data);
    }
    close(fd);
    return reply;
}

// "OUTPUT X Y W H", or "none".
std::optional<OutputCaret> parse(const std::optional<std::string> &reply) {
    if (!reply) {
        return std::nullopt;
    }
    char output[64];
    OutputCaret caret;
    if (std::sscanf(reply->c_str(), "%63s %d %d %d %d", output, &caret.x, &caret.y, &caret.w, &caret.h) != 5) {
        return std::nullopt;
    }
    caret.output = output;
    return caret;
}

} // namespace

std::optional<OutputCaret> textInputCaret() { return parse(request("remotecaret")); }

std::optional<OutputCaret> mapSurfaceCaret(int x, int y, int w, int h, double scale) {
    return parse(request(std::format("remotecaret {} {} {} {} {}", x, y, w, h, scale)));
}
