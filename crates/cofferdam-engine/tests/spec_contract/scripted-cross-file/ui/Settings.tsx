// Inside ui/** but does NOT import 'localStorage' — rule passes.
import * as React from 'react';

export function Settings() {
  return React.createElement('div', null, 'settings');
}
