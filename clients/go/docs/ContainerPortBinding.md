# ContainerPortBinding

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**HostIp** | Pointer to **NullableString** | Host IP that maps to the container port. Empty / &#x60;\&quot;0.0.0.0\&quot;&#x60; means \&quot;any IPv4 address\&quot;. | [optional] 
**HostPort** | Pointer to **NullableString** | Host port (always serialised as a string in Docker&#39;s wire format). | [optional] 

## Methods

### NewContainerPortBinding

`func NewContainerPortBinding() *ContainerPortBinding`

NewContainerPortBinding instantiates a new ContainerPortBinding object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerPortBindingWithDefaults

`func NewContainerPortBindingWithDefaults() *ContainerPortBinding`

NewContainerPortBindingWithDefaults instantiates a new ContainerPortBinding object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetHostIp

`func (o *ContainerPortBinding) GetHostIp() string`

GetHostIp returns the HostIp field if non-nil, zero value otherwise.

### GetHostIpOk

`func (o *ContainerPortBinding) GetHostIpOk() (*string, bool)`

GetHostIpOk returns a tuple with the HostIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHostIp

`func (o *ContainerPortBinding) SetHostIp(v string)`

SetHostIp sets HostIp field to given value.

### HasHostIp

`func (o *ContainerPortBinding) HasHostIp() bool`

HasHostIp returns a boolean if a field has been set.

### SetHostIpNil

`func (o *ContainerPortBinding) SetHostIpNil(b bool)`

 SetHostIpNil sets the value for HostIp to be an explicit nil

### UnsetHostIp
`func (o *ContainerPortBinding) UnsetHostIp()`

UnsetHostIp ensures that no value is present for HostIp, not even an explicit nil
### GetHostPort

`func (o *ContainerPortBinding) GetHostPort() string`

GetHostPort returns the HostPort field if non-nil, zero value otherwise.

### GetHostPortOk

`func (o *ContainerPortBinding) GetHostPortOk() (*string, bool)`

GetHostPortOk returns a tuple with the HostPort field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHostPort

`func (o *ContainerPortBinding) SetHostPort(v string)`

SetHostPort sets HostPort field to given value.

### HasHostPort

`func (o *ContainerPortBinding) HasHostPort() bool`

HasHostPort returns a boolean if a field has been set.

### SetHostPortNil

`func (o *ContainerPortBinding) SetHostPortNil(b bool)`

 SetHostPortNil sets the value for HostPort to be an explicit nil

### UnsetHostPort
`func (o *ContainerPortBinding) UnsetHostPort()`

UnsetHostPort ensures that no value is present for HostPort, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


