import { useSettingsMotion } from "../../hooks/useSettingsMotion.ts";

interface DisplayNameRecord {
  id: string | null;
  name: string;
  scope: string;
  editable: boolean;
}
interface Props {
  records: DisplayNameRecord[];
  editing: boolean;
  saving: boolean;
  removingScope: string | null | undefined;
  onRemove: (id: string | null) => void;
}

function RecordsTable({
  records,
  editing,
  saving,
  removingScope,
  onRemove,
}: Props) {
  const rows = useSettingsMotion<HTMLTableSectionElement>();
  return (
    <table
      class="displayname-records"
      data-editing={editing}
      aria-label="Configured display names"
    >
      <thead class="visually-hidden">
        <tr>
          <th scope="col">Name</th>
          <th scope="col">Scope</th>
          <th scope="col" aria-hidden={!editing}>
            Actions
          </th>
        </tr>
      </thead>
      <tbody ref={rows}>
        {records.map((record) => (
          <tr
            key={record.id ?? "@global"}
            data-displayname-scope={record.id ?? "@global"}
          >
            <td class="displayname-record-name">{record.name}</td>
            <td class="displayname-record-scope">
              <span title={record.scope}>{record.scope}</span>
            </td>
            <td
              class="displayname-record-action"
              aria-hidden={!editing}
              inert={!editing}
            >
              <div class="displayname-record-action-reveal">
                <button
                  type="button"
                  class="displayname-record-remove"
                  aria-label={`Remove display name at ${record.scope}`}
                  aria-busy={saving && removingScope === record.id}
                  disabled={!editing || saving || !record.editable}
                  onClick={() => onRemove(record.id)}
                >
                  Remove
                </button>
              </div>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function DisplayNameRecords(props: Props) {
  const container = useSettingsMotion<HTMLDivElement>();
  return (
    <div class="displayname-records-container" ref={container}>
      {props.records.length ? (
        <RecordsTable {...props} />
      ) : (
        <p>Display name is unset.</p>
      )}
    </div>
  );
}
