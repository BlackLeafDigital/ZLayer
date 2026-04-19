
# RegisteredServiceInfo

Information about a registered service in a tunnel

## Properties

Name | Type
------------ | -------------
`localPort` | number
`name` | string
`protocol` | string
`remotePort` | number
`serviceId` | string
`status` | string

## Example

```typescript
import type { RegisteredServiceInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "localPort": null,
  "name": null,
  "protocol": null,
  "remotePort": null,
  "serviceId": null,
  "status": null,
} satisfies RegisteredServiceInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as RegisteredServiceInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


