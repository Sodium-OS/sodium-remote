// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Headless test of the Remote engine with fcitx5's test frontend: a session switches
// every input context to "remote" and gives each its own engine back, and preedit and
// commit reach the focused field. The D-Bus side is exercised in nested Hyprland by
// spikes/remote-ime/ime-demo.sh.
#include <fcitx-utils/eventdispatcher.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/macros.h>
#include <fcitx-utils/testing.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputmethodgroup.h>
#include <fcitx/inputmethodmanager.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include "remote_public.h"
#include "testfrontend_public.h"

using namespace fcitx;

namespace {

bool groupHasRemote(Instance *instance) {
    for (const auto &item : instance->inputMethodManager().currentGroup().inputMethodList()) {
        if (item.name() == "remote") {
            return true;
        }
    }
    return false;
}

void run(Instance *instance) {
    auto *remote = instance->addonManager().addon("remote", true);
    auto *frontend = instance->addonManager().addon("testfrontend", true);
    FCITX_ASSERT(remote);
    FCITX_ASSERT(frontend);

    // The user's group: "testim" first (the inactive state, like keyboard-us), "other"
    // standing in for Rime.
    auto group = instance->inputMethodManager().currentGroup();
    group.inputMethodList().clear();
    group.inputMethodList().emplace_back("testim");
    group.inputMethodList().emplace_back("other");
    group.setDefaultInputMethod("other");
    instance->inputMethodManager().setGroup(group);

    auto &icManager = instance->inputContextManager();
    auto *a = icManager.findByUUID(frontend->call<ITestFrontend::createInputContext>("a"));
    auto *b = icManager.findByUUID(frontend->call<ITestFrontend::createInputContext>("b"));
    a->focusIn();
    instance->setCurrentInputMethod(a, "other", true);
    FCITX_ASSERT(instance->inputMethod(a) == "other");
    FCITX_ASSERT(instance->inputMethod(b) == "testim");

    // Boundary: Deactivate with no session changes nothing.
    remote->call<IRemoteIme::deactivateSession>();
    FCITX_ASSERT(instance->inputMethod(a) == "other");

    remote->call<IRemoteIme::activateSession>();
    FCITX_ASSERT(groupHasRemote(instance));
    FCITX_ASSERT(instance->inputMethod(a) == "remote") << instance->inputMethod(a);
    FCITX_ASSERT(instance->inputMethod(b) == "remote");
    // A field created during the session joins it.
    auto *c = icManager.findByUUID(frontend->call<ITestFrontend::createInputContext>("c"));
    FCITX_ASSERT(instance->inputMethod(c) == "remote");

    // Preedit then commit into the focused field. The test frontend's context draws no
    // preedit itself, so the preedit shows in fcitx5's panel.
    FCITX_ASSERT(remote->call<IRemoteIme::setPreedit>("ni hao", 6));
    FCITX_ASSERT(a->inputPanel().preedit().toString() == "ni hao");
    FCITX_ASSERT(a->inputPanel().preedit().cursor() == 6);
    // The cursor counts characters, not bytes.
    FCITX_ASSERT(remote->call<IRemoteIme::setPreedit>("你好", 1));
    FCITX_ASSERT(a->inputPanel().preedit().cursor() == 3);
    frontend->call<ITestFrontend::pushCommitExpectation>("你好");
    FCITX_ASSERT(remote->call<IRemoteIme::commit>("你好"));
    FCITX_ASSERT(a->inputPanel().preedit().empty());

    // Boundaries: text that isn't UTF-8, and no focused field.
    FCITX_ASSERT(!remote->call<IRemoteIme::setPreedit>("\xff", 0));
    a->focusOut();
    FCITX_ASSERT(!remote->call<IRemoteIme::commit>("x"));

    remote->call<IRemoteIme::deactivateSession>();
    FCITX_ASSERT(!groupHasRemote(instance));
    FCITX_ASSERT(instance->inputMethod(a) == "other") << instance->inputMethod(a);
    FCITX_ASSERT(instance->inputMethod(b) == "testim");
    FCITX_ASSERT(instance->inputMethod(c) == "testim");

    instance->exit();
}

} // namespace

int main() {
    // Never reach the live compositor from a test.
    unsetenv("HYPRLAND_INSTANCE_SIGNATURE");
    setupTestingEnvironment(REMOTE_BUILD_DIR, {"."}, {"data", REMOTE_TEST_DATA_DIR});
    char arg0[] = "testremote";
    char arg1[] = "--disable=all";
    char arg2[] = "--enable=testim,testfrontend,testui,remote";
    char *argv[] = {arg0, arg1, arg2};
    Log::setLogRule("default=4,remote=5");
    Instance instance(FCITX_ARRAY_SIZE(argv), argv);
    instance.addonManager().registerDefaultLoader(nullptr);
    EventDispatcher dispatcher;
    dispatcher.attach(&instance.eventLoop());
    dispatcher.schedule([&instance]() { run(&instance); });
    instance.exec();
    return 0;
}
