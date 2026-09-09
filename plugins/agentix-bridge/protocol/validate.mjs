import schema from './schema.json' with { type: 'json' };
// Deliberately accepts additional fields so v2 hosts can add optional information.
function matches(spec, value) {
    if (spec.$ref) return matches(schema.$defs[spec.$ref.split('/').at(-1)], value);
    if (spec.oneOf) return spec.oneOf.filter(variant => matches(variant, value)).length === 1;
    if (spec.enum) return spec.enum.includes(value);
    if (Array.isArray(spec.type)) return spec.type.some(type => matches({ ...spec, type }, value));
    switch (spec.type) {
    case 'null': return value === null;
    case 'string': return typeof value === 'string';
    case 'boolean': return typeof value === 'boolean';
    case 'integer': return Number.isSafeInteger(value) && (spec.minimum == null || value >= spec.minimum);
    case 'array': return Array.isArray(value) && value.every(item => matches(spec.items, item));
    case 'object':
        return value !== null && typeof value === 'object' && !Array.isArray(value)
            && (spec.required ?? []).every(key => Object.hasOwn(value, key))
            && Object.entries(spec.properties ?? {}).every(([key, property]) => !Object.hasOwn(value, key) || matches(property, value[key]));
    default: throw new Error(`Unsupported protocol schema: ${spec.type}`);
    }
}
export function validate(type, value) {
    if (!schema.$defs[type]) throw new Error(`Unknown protocol type: ${type}`);
    return matches(schema.$defs[type], value);
}
