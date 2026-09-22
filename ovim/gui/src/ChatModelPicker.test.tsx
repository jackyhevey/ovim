/** @vitest-environment jsdom */
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, expect, it, vi } from "vitest";
import ChatModelPicker from "./ChatModelPicker";

afterEach(cleanup);

const profiles = [
    {
        id: "codex_sol",
        label: "Sol",
        provider: "codex",
        model: "gpt-5.6-sol",
    },
    {
        id: "codex_terra",
        label: "Terra",
        provider: "codex",
        model: "gpt-5.6-terra",
    },
    ...[
        "default",
        "claude-sonnet-5",
        "claude-opus-5",
        "claude-haiku-4-5-20251001",
    ].map((model) => ({
        id: "claude_code",
        label: "Claude Agent",
        provider: "claude_code",
        model,
    })),
];

it("selects provider, model, effort, and permissions through separate controls", async () => {
    const onProfile = vi.fn();
    const onReasoningEffort = vi.fn();
    const onPermissionMode = vi.fn();
    const focusInput = vi.fn();
    const [profile, setProfile] = createSignal("claude_code");
    const [model, setModel] = createSignal("default");

    render(() => (
        <ChatModelPicker
            profile={profile()}
            model={model()}
            profiles={profiles}
            reasoningEffort="high"
            reasoningEffortSelection="default"
            reasoningEffortDefault="high"
            reasoningEfforts={["default", "low", "high"]}
            permissionMode="auto"
            permissionModes={[
                {
                    id: "auto",
                    label: "Auto",
                    description: "Classify permission prompts",
                },
                {
                    id: "dontAsk",
                    label: "Don't ask",
                    description: "Deny calls that require approval",
                },
            ]}
            onProfile={(nextProfile, nextModel) => {
                onProfile(nextProfile, nextModel);
                setProfile(nextProfile);
                setModel(nextModel!);
            }}
            onReasoningEffort={onReasoningEffort}
            onPermissionMode={onPermissionMode}
            focusInput={focusInput}
        />
    ));

    fireEvent.click(
        screen.getByTitle("Configure AI provider, model, and run settings"),
    );
    expect(
        (screen.getByLabelText("AI provider") as HTMLSelectElement).value,
    ).toBe("claude_code");
    expect((screen.getByLabelText("AI model") as HTMLSelectElement).value).toBe(
        JSON.stringify(["claude_code", "default"]),
    );

    fireEvent.change(screen.getByLabelText("AI provider"), {
        target: { value: "codex" },
    });
    expect(onProfile).toHaveBeenCalledWith("codex_sol", "gpt-5.6-sol");
    expect((screen.getByLabelText("AI model") as HTMLSelectElement).value).toBe(
        JSON.stringify(["codex_sol", "gpt-5.6-sol"]),
    );

    fireEvent.change(screen.getByLabelText("AI model"), {
        target: { value: JSON.stringify(["codex_terra", "gpt-5.6-terra"]) },
    });
    expect(onProfile).toHaveBeenLastCalledWith("codex_terra", "gpt-5.6-terra");
    fireEvent.change(screen.getByLabelText("Reasoning effort"), {
        target: { value: "low" },
    });
    expect(onReasoningEffort).toHaveBeenCalledWith("low");
    fireEvent.change(screen.getByLabelText("Permission mode"), {
        target: { value: "dontAsk" },
    });
    expect(onPermissionMode).toHaveBeenCalledWith("dontAsk");

    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await Promise.resolve();
    expect(focusInput).toHaveBeenCalledOnce();
});

it("omits effort and permissions when the selected model does not support them", () => {
    render(() => (
        <ChatModelPicker
            profile="claude_code"
            model="claude-haiku-4-5-20251001"
            profiles={profiles}
            reasoningEffort="default"
            reasoningEffortSelection="default"
            reasoningEfforts={["default"]}
            permissionModes={[]}
            focusInput={() => {}}
        />
    ));
    fireEvent.click(
        screen.getByTitle("Configure AI provider, model, and run settings"),
    );
    expect(screen.queryByLabelText("Reasoning effort")).toBeNull();
    expect(screen.queryByLabelText("Permission mode")).toBeNull();
});
