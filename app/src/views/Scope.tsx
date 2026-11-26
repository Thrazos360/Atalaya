export type Option = { id: string; name: string };

/** «Todos» o una lista elegida: proyectos o IA. */
export function Scope({
  label,
  all,
  value,
  options,
  disabled,
  onChange,
}: {
  label: string;
  all: string;
  value: string[] | null;
  options: Option[];
  disabled: boolean;
  onChange: (next: string[] | null) => void;
}) {
  const picked = value ?? [];
  return (
    <fieldset className="skill-scope" disabled={disabled}>
      <legend>{label}</legend>
      <label className="check">
        <input type="checkbox" checked={value === null} onChange={(e) => onChange(e.target.checked ? null : [])} /> {all}
      </label>
      {value !== null &&
        options.map((o) => (
          <label key={o.id} className="check">
            <input
              type="checkbox"
              checked={picked.includes(o.id)}
              onChange={(e) => onChange(e.target.checked ? [...picked, o.id] : picked.filter((p) => p !== o.id))}
            />{" "}
            {o.name}
          </label>
        ))}
      {value !== null && !picked.length && <span className="stamp">Ninguno elegido: no llega a nadie.</span>}
    </fieldset>
  );
}
