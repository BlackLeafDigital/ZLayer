
# NotifierConfig

Channel-specific configuration for a notifier.

## Properties

Name | Type
------------ | -------------
`type` | string
`webhookUrl` | string
`headers` | { [key: string]: string; }
`method` | string
`url` | string
`from` | string
`host` | string
`password` | string
`port` | number
`to` | Array&lt;string&gt;
`username` | string

## Example

```typescript
import type { NotifierConfig } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "type": null,
  "webhookUrl": null,
  "headers": null,
  "method": null,
  "url": null,
  "from": null,
  "host": null,
  "password": null,
  "port": null,
  "to": null,
  "username": null,
} satisfies NotifierConfig

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NotifierConfig
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


