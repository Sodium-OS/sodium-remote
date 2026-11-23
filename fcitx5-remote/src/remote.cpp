// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#include "remote.h"

#include <algorithm>
#include <cstdint>
#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/dbus/bus.h>
#include <fcitx-utils/dbus/objectvtable.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/textformatflags.h>
#include <fcitx-utils/utf8.h>
#include <fcitx/event.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputmethodgroup.h>
#include <fcitx/inputmethodmanager.h>
#include <fcitx/inputpanel.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>
#include "dbus_public.h"

namespace {

FCITX_DEFINE_LOG_CATEGORY(remote_log, "remote");
#define REMOTE_DEBUG() FCITX_LOGC(remote_log, Debug)
#define REMOTE_INFO() FCITX_LOGC(remote_log, Info)

constexpr const char *busName = "org.sodiumos.Remote.Ime";
constexpr const char *objectPath = "/org/sodiumos/Remote/Ime";
constexpr const char *interfaceName = "org.sodiumos.Remote.Ime";

bool contains(const fcitx::InputMethodGroup &group, const std::string &name) {
    const auto &list = group.inputMethodList();
    return std::any_of(list.begin(), list.end(),
                       [&name](const auto &item) { return item.name() == name; });
}

bool isPassword(fcitx::InputContext *ic) {
    return ic->capabilityFlags().test(fcitx::CapabilityFlag::Password);
}

} // namespace

// org.sodiumos.Remote.Ime on fcitx5's own session bus connection.
class RemoteDBus final : public fcitx::dbus::ObjectVTable<RemoteDBus> {
public:
    explicit RemoteDBus(RemoteIme *ime) : ime_(ime) {}

    bool setPreedit(const std::string &text, int32_t cursor) { return ime_->setPreedit(text, cursor); }
    bool commit(const std::string &text) { return ime_->commit(text); }
    void activate() { ime_->activateSession(); }
    void deactivate() { ime_->deactivateSession(); }
    bool active() { return ime_->sessionActive(); }

    // The focused field's caret, in pixels of the output it is on (REMOTE-1).
    FCITX_OBJECT_VTABLE_SIGNAL(caretRect, "CaretRect", "iiii");
    // A text field gained or lost focus; password fields want secure input on the Mac.
    FCITX_OBJECT_VTABLE_SIGNAL(focusChanged, "FocusChanged", "bb");
    // The app dropped the composition (focus moved, the user clicked in the text):
    // the Mac's input method should discard its marked text.
    FCITX_OBJECT_VTABLE_SIGNAL(reset, "Reset", "");

private:
    RemoteIme *ime_;
    FCITX_OBJECT_VTABLE_METHOD(setPreedit, "SetPreedit", "si", "b");
    FCITX_OBJECT_VTABLE_METHOD(commit, "Commit", "s", "b");
    FCITX_OBJECT_VTABLE_METHOD(activate, "Activate", "", "");
    FCITX_OBJECT_VTABLE_METHOD(deactivate, "Deactivate", "", "");
    FCITX_OBJECT_VTABLE_PROPERTY(activeProperty, "Active", "b", [this]() { return active(); });
};

RemoteIme::RemoteIme(fcitx::Instance *instance) : instance_(instance) {
    using fcitx::EventType;
    using fcitx::EventWatcherPhase;

    if (auto *dbusAddon = dbus()) {
        auto *bus = dbusAddon->call<fcitx::IDBusModule::bus>();
        dbus_ = std::make_unique<RemoteDBus>(this);
        bus->addObjectVTable(objectPath, interfaceName, *dbus_);
        if (!bus->requestName(busName, fcitx::Flags<fcitx::dbus::RequestNameFlag>{
                                           fcitx::dbus::RequestNameFlag::AllowReplacement,
                                           fcitx::dbus::RequestNameFlag::ReplaceExisting})) {
            FCITX_LOGC(remote_log, Warn) << "could not own " << busName;
        }
    }

    // New windows during a session use this engine too.
    watchers_.emplace_back(instance_->watchEvent(
        EventType::InputContextCreated, EventWatcherPhase::PostInputMethod,
        [this](fcitx::Event &event) {
            if (active_) {
                switchToRemote(static_cast<fcitx::InputContextEvent &>(event).inputContext());
            }
        }));
    watchers_.emplace_back(instance_->watchEvent(
        EventType::InputContextFocusIn, EventWatcherPhase::PostInputMethod,
        [this](fcitx::Event &event) {
            auto *ic = static_cast<fcitx::InputContextEvent &>(event).inputContext();
            reportFocus(ic);
            reportCaret(ic);
        }));
    watchers_.emplace_back(instance_->watchEvent(
        EventType::InputContextFocusOut, EventWatcherPhase::PostInputMethod,
        [this](fcitx::Event &) {
            // Focus may move straight to another field; report what holds focus now.
            reportFocus(focusedInputContext());
        }));
    watchers_.emplace_back(instance_->watchEvent(
        EventType::InputContextCapabilityChanged, EventWatcherPhase::PostInputMethod,
        [this](fcitx::Event &event) {
            auto *ic = static_cast<fcitx::InputContextEvent &>(event).inputContext();
            if (ic->hasFocus()) {
                reportFocus(ic);
            }
        }));
    // The caret moves: D-Bus frontend apps send a new rect; text-input apps send new
    // surrounding text (a click, a key), and their rect goes to the compositor.
    for (auto type : {EventType::InputContextCursorRectChanged, EventType::InputContextSurroundingTextUpdated}) {
        watchers_.emplace_back(instance_->watchEvent(
            type, EventWatcherPhase::PostInputMethod, [this](fcitx::Event &event) {
                auto *ic = static_cast<fcitx::InputContextEvent &>(event).inputContext();
                if (ic->hasFocus()) {
                    reportCaret(ic);
                }
            }));
    }

    // The engine joins the user's input method group only for the length of a session,
    // and a session never outlives fcitx5. If fcitx5 saved its profile mid-session (it
    // exited, or a config tool saved), the group still lists us: take it out again once
    // the profile has loaded.
    groupChanged_ = instance_->inputMethodManager().connect<fcitx::InputMethodManager::CurrentGroupChanged>(
        [this](const std::string &) {
            if (cleanedUp_ || active_ || cleanup_) {
                return;
            }
            cleanup_ = instance_->eventLoop().addDeferEvent([this](fcitx::EventSource *) {
                cleanedUp_ = true;
                if (!active_) {
                    removeFromAllGroups();
                }
                cleanup_.reset();
                return true;
            });
        });
}

RemoteIme::~RemoteIme() = default;

std::vector<fcitx::InputMethodEntry> RemoteIme::listInputMethods() {
    std::vector<fcitx::InputMethodEntry> result;
    result.emplace_back(imName, "Remote", "", "remote");
    result.back().setLabel("⌘").setIcon("input-keyboard").setConfigurable(false);
    return result;
}

void RemoteIme::reset(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) {
    dropPreedit(event.inputContext());
}

void RemoteIme::deactivate(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) {
    dropPreedit(event.inputContext());
}

fcitx::InputContext *RemoteIme::focusedInputContext() const {
    auto *ic = instance_->inputContextManager().lastFocusedInputContext();
    return ic && ic->hasFocus() ? ic : nullptr;
}

void RemoteIme::switchToRemote(fcitx::InputContext *ic) {
    auto engine = instance_->inputMethod(ic);
    if (engine == imName) {
        return;
    }
    previous_.emplace(ic->uuid(), engine);
    instance_->setCurrentInputMethod(ic, imName, true);
}

void RemoteIme::activateSession() {
    if (active_) {
        return;
    }
    auto &manager = instance_->inputMethodManager();
    auto &icManager = instance_->inputContextManager();

    // Remember every input context's engine before the group change resets them all.
    previous_.clear();
    icManager.foreach([this](fcitx::InputContext *ic) {
        previous_[ic->uuid()] = instance_->inputMethod(ic);
        return true;
    });

    active_ = true;
    auto group = manager.currentGroup();
    if (!contains(group, imName)) {
        group.inputMethodList().emplace_back(imName);
        manager.setGroup(std::move(group));
    }
    icManager.foreach([this](fcitx::InputContext *ic) {
        instance_->setCurrentInputMethod(ic, imName, true);
        return true;
    });
    REMOTE_INFO() << "session on: " << previous_.size() << " input contexts switched";
}

void RemoteIme::deactivateSession() {
    if (!active_) {
        return;
    }
    active_ = false;
    auto &manager = instance_->inputMethodManager();

    // Leaving the group resets every input context to the group's first (inactive)
    // input method; then each gets its own engine back.
    auto group = manager.currentGroup();
    auto &list = group.inputMethodList();
    list.erase(std::remove_if(list.begin(), list.end(),
                              [](const auto &item) { return item.name() == imName; }),
               list.end());
    manager.setGroup(group);

    int restored = 0;
    instance_->inputContextManager().foreach([this, &group, &restored](fcitx::InputContext *ic) {
        auto iter = previous_.find(ic->uuid());
        if (iter != previous_.end() && contains(group, iter->second)) {
            instance_->setCurrentInputMethod(ic, iter->second, true);
            restored++;
        }
        return true;
    });
    previous_.clear();
    REMOTE_INFO() << "session off: " << restored << " input contexts restored";
}

void RemoteIme::showPreedit(fcitx::InputContext *ic, const fcitx::Text &text) {
    auto &panel = ic->inputPanel();
    // Inline in the app when it draws preedit itself, else in fcitx5's own panel.
    if (ic->capabilityFlags().test(fcitx::CapabilityFlag::Preedit)) {
        panel.setClientPreedit(text);
        panel.setPreedit({});
    } else {
        panel.setPreedit(text);
        panel.setClientPreedit({});
    }
    ic->updatePreedit();
    ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
}

bool RemoteIme::setPreedit(const std::string &text, int cursor) {
    auto *ic = focusedInputContext();
    if (!ic || !fcitx::utf8::validate(text)) {
        return false;
    }
    if (auto *old = preeditIc_.get(); old && old != ic) {
        showPreedit(old, {});
    }

    fcitx::Text preedit;
    if (!text.empty()) {
        preedit.append(text, fcitx::TextFormatFlag::Underline);
        if (cursor >= 0) {
            auto chars = std::min<size_t>(cursor, fcitx::utf8::length(text));
            preedit.setCursor(fcitx::utf8::ncharByteLength(text.begin(), chars));
        }
    }
    showPreedit(ic, preedit);
    preeditIc_ = text.empty() ? fcitx::TrackableObjectReference<fcitx::InputContext>() : ic->watch();
    // The caret moves with the preedit, and the Mac places its candidates there.
    reportCaret(ic);
    return true;
}

bool RemoteIme::commit(const std::string &text) {
    auto *ic = focusedInputContext();
    if (!ic || !fcitx::utf8::validate(text)) {
        return false;
    }
    if (preeditIc_.get() == ic) {
        showPreedit(ic, {});
        preeditIc_.unwatch();
    }
    if (!text.empty()) {
        ic->commitString(text);
    }
    reportCaret(ic);
    return true;
}

void RemoteIme::dropPreedit(fcitx::InputContext *ic) {
    if (!ic || preeditIc_.get() != ic) {
        return;
    }
    preeditIc_.unwatch();
    ic->inputPanel().reset();
    ic->updatePreedit();
    ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
    if (dbus_) {
        dbus_->reset();
    }
}

void RemoteIme::reportFocus(fcitx::InputContext *ic) {
    bool focused = ic != nullptr;
    bool password = focused && isPassword(ic);
    if (focused == lastFocused_ && password == lastPassword_) {
        return;
    }
    lastFocused_ = focused;
    lastPassword_ = password;
    if (dbus_) {
        dbus_->focusChanged(focused, password);
    }
}

void RemoteIme::reportCaret(fcitx::InputContext *ic) {
    if (!dbus_) {
        return;
    }
    if (ic->frontendName() == "wayland_v2") {
        // A text-input-v3 app tells only the compositor where its caret is, and only once
        // it has drawn the change: ask after its next frame or two.
        caretTimer_ = instance_->eventLoop().addTimeEvent(
            CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + 50000, 0,
            [this](fcitx::EventSourceTime *, uint64_t) {
                sendCaret(textInputCaret());
                return true;
            });
        return;
    }
    const auto &rect = ic->cursorRect();
    REMOTE_DEBUG() << "caret " << rect << " frontend=" << ic->frontendName() << " scale=" << ic->scaleFactor()
                   << " relative=" << ic->capabilityFlags().test(fcitx::CapabilityFlag::RelativeRect);
    // Over the D-Bus frontend on Wayland, Qt's and GTK's fcitx modules give the caret
    // relative to their window's surface. (Absolute ones come from X11 apps: not mapped.)
    // A caret is often zero pixels wide; it needs a height.
    if (rect.height() <= 0 || !ic->capabilityFlags().test(fcitx::CapabilityFlag::RelativeRect)) {
        return;
    }
    sendCaret(mapSurfaceCaret(rect.left(), rect.top(), rect.width(), rect.height(), ic->scaleFactor()));
}

void RemoteIme::sendCaret(const std::optional<OutputCaret> &caret) {
    if (!caret || caret == lastCaret_) {
        return;
    }
    lastCaret_ = caret;
    REMOTE_DEBUG() << "CaretRect " << caret->output << " " << caret->x << "," << caret->y << " "
                   << caret->w << "x" << caret->h;
    dbus_->caretRect(caret->x, caret->y, caret->w, caret->h);
}

void RemoteIme::removeFromAllGroups() {
    auto &manager = instance_->inputMethodManager();
    for (const auto &name : manager.groups()) {
        const auto *group = manager.group(name);
        if (!group || !contains(*group, imName)) {
            continue;
        }
        auto copy = *group;
        auto &list = copy.inputMethodList();
        list.erase(std::remove_if(list.begin(), list.end(),
                                  [](const auto &item) { return item.name() == imName; }),
                   list.end());
        manager.setGroup(std::move(copy));
        FCITX_LOGC(remote_log, Warn) << "removed a leftover \"" << imName << "\" from group " << name;
    }
}

FCITX_ADDON_FACTORY_V2(remote, RemoteImeFactory);
