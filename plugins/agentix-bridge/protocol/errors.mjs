export const failure = (code, message) => Object.assign(new Error(message), { bridgeCode: code });
