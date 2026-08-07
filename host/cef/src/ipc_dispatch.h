#pragma once

#include "ipc/gen/cef_ipc.pb.h"

class BrowserApp;

// Host → CEF Envelope dispatch (proto path). Must run on CEF UI thread.
void DispatchEnvelope(BrowserApp& app, const gameoverlay::cef::Envelope& env);
