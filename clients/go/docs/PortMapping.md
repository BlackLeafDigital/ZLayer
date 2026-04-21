# PortMapping

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ContainerPort** | **int32** | Container-side port. | 
**HostIp** | Pointer to **string** | Host interface to bind on. Defaults to &#x60;\&quot;0.0.0.0\&quot;&#x60; (all interfaces). | [optional] 
**HostPort** | Pointer to **NullableInt32** | Host port. &#x60;None&#x60; (or zero) means \&quot;assign an ephemeral port\&quot;. | [optional] 
**Protocol** | Pointer to [**PortProtocol**](PortProtocol.md) | Transport protocol (defaults to TCP). | [optional] 

## Methods

### NewPortMapping

`func NewPortMapping(containerPort int32, ) *PortMapping`

NewPortMapping instantiates a new PortMapping object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewPortMappingWithDefaults

`func NewPortMappingWithDefaults() *PortMapping`

NewPortMappingWithDefaults instantiates a new PortMapping object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetContainerPort

`func (o *PortMapping) GetContainerPort() int32`

GetContainerPort returns the ContainerPort field if non-nil, zero value otherwise.

### GetContainerPortOk

`func (o *PortMapping) GetContainerPortOk() (*int32, bool)`

GetContainerPortOk returns a tuple with the ContainerPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerPort

`func (o *PortMapping) SetContainerPort(v int32)`

SetContainerPort sets ContainerPort field to given value.


### GetHostIp

`func (o *PortMapping) GetHostIp() string`

GetHostIp returns the HostIp field if non-nil, zero value otherwise.

### GetHostIpOk

`func (o *PortMapping) GetHostIpOk() (*string, bool)`

GetHostIpOk returns a tuple with the HostIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHostIp

`func (o *PortMapping) SetHostIp(v string)`

SetHostIp sets HostIp field to given value.

### HasHostIp

`func (o *PortMapping) HasHostIp() bool`

HasHostIp returns a boolean if a field has been set.

### GetHostPort

`func (o *PortMapping) GetHostPort() int32`

GetHostPort returns the HostPort field if non-nil, zero value otherwise.

### GetHostPortOk

`func (o *PortMapping) GetHostPortOk() (*int32, bool)`

GetHostPortOk returns a tuple with the HostPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHostPort

`func (o *PortMapping) SetHostPort(v int32)`

SetHostPort sets HostPort field to given value.

### HasHostPort

`func (o *PortMapping) HasHostPort() bool`

HasHostPort returns a boolean if a field has been set.

### SetHostPortNil

`func (o *PortMapping) SetHostPortNil(b bool)`

 SetHostPortNil sets the value for HostPort to be an explicit nil

### UnsetHostPort
`func (o *PortMapping) UnsetHostPort()`

UnsetHostPort ensures that no value is present for HostPort, not even an explicit nil
### GetProtocol

`func (o *PortMapping) GetProtocol() PortProtocol`

GetProtocol returns the Protocol field if non-nil, zero value otherwise.

### GetProtocolOk

`func (o *PortMapping) GetProtocolOk() (*PortProtocol, bool)`

GetProtocolOk returns a tuple with the Protocol field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProtocol

`func (o *PortMapping) SetProtocol(v PortProtocol)`

SetProtocol sets Protocol field to given value.

### HasProtocol

`func (o *PortMapping) HasProtocol() bool`

HasProtocol returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


