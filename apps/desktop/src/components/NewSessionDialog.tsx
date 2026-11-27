import { type KeyboardEvent, type SyntheticEvent, useMemo, useState } from "react";

import type { SessionKind } from "../lib/ipc";
import { useWorkbench, useWorkspace } from "../workspace/context";
import { describeCommandError, folderName, KIND_LABELS } from "../workspace/describe";
import { CloseIcon, KindIcon } from "./icons";
import { Modal } from "./Modal";

const KINDS: SessionKind[] = ["claude", "shell", "background"];

/** `null` stands for the home folder, which the Rust side picks. */
type Folder = string | null;

/**
 * Starts a session. Made for the keyboard: it opens on the list of recent
 * folders with the most recent one chosen and Claude Code as the kind, so
 * Enter alone starts Claude Code where the user last worked. Up and Down
 * choose another folder; "Choose folder…" opens the picker of the system.
 * A background command needs its command line, and may get a name.
 */
export function NewSessionDialog() {
  const workbench = useWorkbench();
  const workspace = useWorkspace();
  const recent = useMemo(() => workspace?.recentFolders ?? [], [workspace]);
  const [picked, setPicked] = useState<string[]>([]);
  const folders = useMemo<Folder[]>(() => {
    const list = [...picked, ...recent.filter((folder) => !picked.includes(folder))];
    return list.length > 0 ? list : [null];
  }, [picked, recent]);
  const [chosen, setChosen] = useState<Folder | undefined>(undefined);
  const folder = chosen !== undefined && folders.includes(chosen) ? chosen : folders[0];
  const [kind, setKind] = useState<SessionKind>("claude");
  const [command, setCommand] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const needsCommand = kind === "background" && command.trim() === "";

  const submit = (event?: SyntheticEvent) => {
    event?.preventDefault();
    if (busy) return;
    if (needsCommand) {
      setError("A background session needs a command to run.");
      return;
    }
    setBusy(true);
    setError(null);
    workbench
      .createSession({
        kind,
        ...(folder ? { cwd: folder } : {}),
        ...(kind === "background" ? { command: command.trim() } : {}),
        ...(kind === "background" && name.trim() !== "" ? { name: name.trim() } : {}),
      })
      .catch((failure: unknown) => {
        setError(describeCommandError(failure));
        setBusy(false);
      });
  };

  const choose = () => {
    setError(null);
    workbench
      .pickFolder()
      .then((path) => {
        if (path === null) return;
        setPicked((previous) => [path, ...previous.filter((each) => each !== path)]);
        setChosen(path);
      })
      .catch((failure: unknown) => {
        setError(describeCommandError(failure));
      });
  };

  const onListKey = (event: KeyboardEvent) => {
    const index = folders.indexOf(folder ?? null);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      const next = folders[Math.min(folders.length - 1, Math.max(0, index + step))];
      if (next !== undefined) setChosen(next);
    } else if (event.key === "Enter") {
      event.preventDefault();
      submit();
    } else if ((event.key === "Delete" || event.key === "Backspace") && folder) {
      event.preventDefault();
      forget(folder);
    }
  };

  const forget = (path: string) => {
    setPicked((previous) => previous.filter((each) => each !== path));
    if (recent.includes(path)) void workbench.forgetFolder(path);
  };

  const optionId = (index: number) => `new-session-folder-${String(index)}`;

  return (
    <Modal title="New session" testId="new-session-dialog" onClose={workbench.closeDialog}>
      <form className="new-session" onSubmit={submit}>
        <fieldset className="kind-picker">
          <legend>Kind</legend>
          {KINDS.map((each) => (
            <label key={each} className="kind-option" data-checked={kind === each || undefined}>
              <input
                type="radio"
                name="kind"
                value={each}
                checked={kind === each}
                data-testid={`kind-${each}`}
                onChange={() => {
                  setKind(each);
                  setError(null);
                }}
              />
              <KindIcon kind={each} />
              {KIND_LABELS[each]}
            </label>
          ))}
        </fieldset>

        <div className="folder-picker">
          <span className="field-label" id="new-session-folders-label">
            Folder
          </span>
          <ul
            role="listbox"
            tabIndex={0}
            data-autofocus
            aria-labelledby="new-session-folders-label"
            aria-activedescendant={optionId(folders.indexOf(folder ?? null))}
            className="folder-list"
            data-testid="folder-list"
            onKeyDown={onListKey}
          >
            {folders.map((each, index) => (
              <li
                key={each ?? "~"}
                id={optionId(index)}
                role="option"
                aria-selected={each === folder}
                className="folder-option"
                data-testid="folder-option"
                onClick={() => {
                  setChosen(each);
                }}
                onDoubleClick={() => {
                  setChosen(each);
                  submit();
                }}
              >
                <span className="folder-name">
                  {each === null ? "Home folder" : folderName(each)}
                </span>
                {each !== null && (
                  <span className="folder-path" title={each}>
                    {each}
                  </span>
                )}
                {each !== null && (
                  <button
                    type="button"
                    className="icon-button folder-forget"
                    tabIndex={-1}
                    aria-label={`Remove ${each} from the recent folders`}
                    title="Remove from the recent folders"
                    data-testid="folder-forget"
                    onClick={(event) => {
                      event.stopPropagation();
                      forget(each);
                    }}
                  >
                    <CloseIcon />
                  </button>
                )}
              </li>
            ))}
          </ul>
          <button type="button" className="button" data-testid="choose-folder" onClick={choose}>
            Choose folder…
          </button>
        </div>

        {kind === "background" && (
          <div className="background-fields">
            <label className="field">
              <span className="field-label">Command</span>
              <input
                className="text-input"
                data-testid="background-command"
                value={command}
                required
                aria-invalid={error !== null && needsCommand}
                placeholder="npm run dev"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => {
                  setCommand(event.target.value);
                }}
              />
            </label>
            <label className="field">
              <span className="field-label">Name (optional)</span>
              <input
                className="text-input"
                data-testid="background-name"
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                }}
              />
            </label>
          </div>
        )}

        {error !== null && (
          <p className="form-error" role="alert" data-testid="new-session-error">
            {error}
          </p>
        )}

        <div className="modal-actions">
          <button type="button" className="button" onClick={workbench.closeDialog}>
            Cancel
          </button>
          <button
            type="submit"
            className="button button-primary"
            data-testid="create-session"
            disabled={busy || needsCommand}
          >
            Start {KIND_LABELS[kind]}
          </button>
        </div>
      </form>
    </Modal>
  );
}
