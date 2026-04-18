
# NotifierConfigOneOf3

SMTP email configuration.

## Properties

Name | Type
------------ | -------------
`from` | string
`host` | string
`password` | string
`port` | number
`to` | Array&lt;string&gt;
`type` | string
`username` | string

## Example

```typescript
import type { NotifierConfigOneOf3 } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "from": null,
  "host": null,
  "password": null,
  "port": null,
  "to": null,
  "type": null,
  "username": null,
} satisfies NotifierConfigOneOf3

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NotifierConfigOneOf3
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


