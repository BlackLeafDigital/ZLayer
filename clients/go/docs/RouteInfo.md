# RouteInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Backends** | **[]string** | Backend addresses currently assigned to this route. | 
**Endpoint** | **string** | Endpoint name within the service. | 
**Expose** | **string** | Exposure type (public / internal). | 
**Host** | Pointer to **NullableString** | Host pattern (e.g. &#x60;*.example.com&#x60;), or &#x60;null&#x60; for any host. | [optional] 
**PathPrefix** | **string** | Path prefix matched by this route. | 
**Protocol** | **string** | Protocol (http, https, tcp, etc.). | 
**Service** | **string** | Owning service name. | 
**StripPrefix** | **bool** | Whether the matched prefix is stripped before forwarding. | 
**TargetPort** | **int32** | Container target port. | 

## Methods

### NewRouteInfo

`func NewRouteInfo(backends []string, endpoint string, expose string, pathPrefix string, protocol string, service string, stripPrefix bool, targetPort int32, ) *RouteInfo`

NewRouteInfo instantiates a new RouteInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRouteInfoWithDefaults

`func NewRouteInfoWithDefaults() *RouteInfo`

NewRouteInfoWithDefaults instantiates a new RouteInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBackends

`func (o *RouteInfo) GetBackends() []string`

GetBackends returns the Backends field if non-nil, zero value otherwise.

### GetBackendsOk

`func (o *RouteInfo) GetBackendsOk() (*[]string, bool)`

GetBackendsOk returns a tuple with the Backends field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBackends

`func (o *RouteInfo) SetBackends(v []string)`

SetBackends sets Backends field to given value.


### GetEndpoint

`func (o *RouteInfo) GetEndpoint() string`

GetEndpoint returns the Endpoint field if non-nil, zero value otherwise.

### GetEndpointOk

`func (o *RouteInfo) GetEndpointOk() (*string, bool)`

GetEndpointOk returns a tuple with the Endpoint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEndpoint

`func (o *RouteInfo) SetEndpoint(v string)`

SetEndpoint sets Endpoint field to given value.


### GetExpose

`func (o *RouteInfo) GetExpose() string`

GetExpose returns the Expose field if non-nil, zero value otherwise.

### GetExposeOk

`func (o *RouteInfo) GetExposeOk() (*string, bool)`

GetExposeOk returns a tuple with the Expose field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpose

`func (o *RouteInfo) SetExpose(v string)`

SetExpose sets Expose field to given value.


### GetHost

`func (o *RouteInfo) GetHost() string`

GetHost returns the Host field if non-nil, zero value otherwise.

### GetHostOk

`func (o *RouteInfo) GetHostOk() (*string, bool)`

GetHostOk returns a tuple with the Host field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHost

`func (o *RouteInfo) SetHost(v string)`

SetHost sets Host field to given value.

### HasHost

`func (o *RouteInfo) HasHost() bool`

HasHost returns a boolean if a field has been set.

### SetHostNil

`func (o *RouteInfo) SetHostNil(b bool)`

 SetHostNil sets the value for Host to be an explicit nil

### UnsetHost
`func (o *RouteInfo) UnsetHost()`

UnsetHost ensures that no value is present for Host, not even an explicit nil
### GetPathPrefix

`func (o *RouteInfo) GetPathPrefix() string`

GetPathPrefix returns the PathPrefix field if non-nil, zero value otherwise.

### GetPathPrefixOk

`func (o *RouteInfo) GetPathPrefixOk() (*string, bool)`

GetPathPrefixOk returns a tuple with the PathPrefix field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPathPrefix

`func (o *RouteInfo) SetPathPrefix(v string)`

SetPathPrefix sets PathPrefix field to given value.


### GetProtocol

`func (o *RouteInfo) GetProtocol() string`

GetProtocol returns the Protocol field if non-nil, zero value otherwise.

### GetProtocolOk

`func (o *RouteInfo) GetProtocolOk() (*string, bool)`

GetProtocolOk returns a tuple with the Protocol field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProtocol

`func (o *RouteInfo) SetProtocol(v string)`

SetProtocol sets Protocol field to given value.


### GetService

`func (o *RouteInfo) GetService() string`

GetService returns the Service field if non-nil, zero value otherwise.

### GetServiceOk

`func (o *RouteInfo) GetServiceOk() (*string, bool)`

GetServiceOk returns a tuple with the Service field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetService

`func (o *RouteInfo) SetService(v string)`

SetService sets Service field to given value.


### GetStripPrefix

`func (o *RouteInfo) GetStripPrefix() bool`

GetStripPrefix returns the StripPrefix field if non-nil, zero value otherwise.

### GetStripPrefixOk

`func (o *RouteInfo) GetStripPrefixOk() (*bool, bool)`

GetStripPrefixOk returns a tuple with the StripPrefix field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStripPrefix

`func (o *RouteInfo) SetStripPrefix(v bool)`

SetStripPrefix sets StripPrefix field to given value.


### GetTargetPort

`func (o *RouteInfo) GetTargetPort() int32`

GetTargetPort returns the TargetPort field if non-nil, zero value otherwise.

### GetTargetPortOk

`func (o *RouteInfo) GetTargetPortOk() (*int32, bool)`

GetTargetPortOk returns a tuple with the TargetPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTargetPort

`func (o *RouteInfo) SetTargetPort(v int32)`

SetTargetPort sets TargetPort field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


