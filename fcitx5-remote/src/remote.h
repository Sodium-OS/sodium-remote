// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The "remote" input method engine for fcitx5.
//
// While a Remote session is on, the Mac's own input method composes text and remoted
// hands it to this engine over D-Bus: the preedit goes inline into the focused app
// through fcitx5's normal frontends, commits are committed, and the caret rectangle goes
// back so the Mac can put its candidate window at the caret in the stream.
#pragma once

#include <map>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include <fcitx-utils/eventloopinterface.h>
#include <fcitx-utils/handlertable.h>
#include <fcitx-utils/rect.h>
#include <fcitx-utils/signals.h>
#include <fcitx-utils/trackableobject.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/instance.h>
#include "compositor.h"
#include "remote_public.h"

class RemoteDBus;

class RemoteIme final : public fcitx::InputMethodEngineV2 {
public:
    // The input method's unique name in fcitx5's group.
    static constexpr const char *imName = "remote";

    explicit RemoteIme(fcitx::Instance *instance);
    ~RemoteIme() override;

    std::vector<fcitx::InputMethodEntry> listInputMethods() override;
    // Keys pass through untouched: the Mac's input method already consumed the ones it
    // wanted, and everything else must reach the app as a normal key.
    void keyEvent(const fcitx::InputMethodEntry &, fcitx::KeyEvent &) override {}
    void reset(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override;
    void deactivate(const fcitx::InputMethodEntry &, fcitx::InputContextEvent &event) override;

    // Switch every input context (and every one created later) to this engine,
    // remembering each one's engine; deactivateSession() gives them back. Both are
    // idempotent.
    void activateSession();
    void deactivateSession();
    // Show text as the focused field's preedit, with the caret after `cursor`
    // characters (code points; -1 hides it). Empty text clears it. False when no field
    // has focus or the text isn't UTF-8.
    bool setPreedit(const std::string &text, int cursor);
    // Clear the preedit and commit text to the focused field.
    bool commit(const std::string &text);

    bool sessionActive() const { return active_; }
    fcitx::Instance *instance() const { return instance_; }

private:
    FCITX_ADDON_DEPENDENCY_LOADER(dbus, instance_->addonManager());
    FCITX_ADDON_EXPORT_FUNCTION(RemoteIme, activateSession);
    FCITX_ADDON_EXPORT_FUNCTION(RemoteIme, deactivateSession);
    FCITX_ADDON_EXPORT_FUNCTION(RemoteIme, setPreedit);
    FCITX_ADDON_EXPORT_FUNCTION(RemoteIme, commit);

    fcitx::InputContext *focusedInputContext() const;
    void switchToRemote(fcitx::InputContext *ic);
    void showPreedit(fcitx::InputContext *ic, const fcitx::Text &text);
    // Drop our preedit from ic, and tell remoted so the Mac discards its composition.
    void dropPreedit(fcitx::InputContext *ic);
    void reportFocus(fcitx::InputContext *ic);
    // Find where ic's caret is on screen (asking the compositor) and send CaretRect.
    void reportCaret(fcitx::InputContext *ic);
    void sendCaret(const std::optional<OutputCaret> &caret);
    void removeFromAllGroups();

    fcitx::Instance *instance_;
    bool active_ = false;
    // Each input context's engine before the session, by input context.
    std::map<fcitx::ICUUID, std::string> previous_;
    // The input context currently showing our preedit, if any.
    fcitx::TrackableObjectReference<fcitx::InputContext> preeditIc_;
    // The last values sent, so a signal goes out only on a change.
    std::optional<OutputCaret> lastCaret_;
    bool lastFocused_ = false;
    bool lastPassword_ = false;

    std::unique_ptr<RemoteDBus> dbus_;
    std::unique_ptr<fcitx::EventSourceTime> caretTimer_;
    std::vector<std::unique_ptr<fcitx::HandlerTableEntry<fcitx::EventHandler>>> watchers_;
    fcitx::ScopedConnection groupChanged_;
    std::unique_ptr<fcitx::EventSource> cleanup_;
    bool cleanedUp_ = false;
};

class RemoteImeFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new RemoteIme(manager->instance());
    }
};
